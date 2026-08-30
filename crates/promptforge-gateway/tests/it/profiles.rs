//! Admin profile routes: switch, list, status, and failure isolation.

use std::fs;

use promptforge_gateway::{Config, Gateway, ProfileName, ProfilesContext};
use serde_json::Value;

use crate::support::{
    PHASE_TIMEOUT, TestServer, catalog_ids, fake_backend, join_within, json_within, parse_sse,
    send_within, text_within,
};

/// Two remote-only profiles, `alpha` (alpha-model) and `beta` (beta-model),
/// with a gateway started on alpha; the tempdir is the profiles directory.
async fn alpha_beta_server() -> (tempfile::TempDir, TestServer) {
    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();
    let profile_toml = |model: &str, context: u32| {
        format!(
            r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "{model}"
description = "{model} catalog entry"
context = {context}
upstream = "backend-model"
endpoints = ["fake"]
"#
        )
    };
    fs::write(
        profiles.path().join("alpha.toml"),
        profile_toml("alpha-model", 8192),
    )
    .unwrap();
    fs::write(
        profiles.path().join("beta.toml"),
        profile_toml("beta-model", 4096),
    )
    .unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;
    (profiles, server)
}

/// Posts a switch for `name` and returns the parsed SSE events, asserting
/// the stream handshake: an accepted switch is 200 `text/event-stream`.
async fn switch_stream_events(
    http: &reqwest::Client,
    addr: std::net::SocketAddr,
    name: &str,
) -> Vec<Value> {
    let response = send_within(
        http.post(format!("http://{addr}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    parse_sse(&text_within(response).await)
}

/// Switch-profile rebuilds the catalog from a remote-only profile (no llama spawn).
#[tokio::test]
async fn switch_profile_updates_models_catalog() {
    let (_profiles, server) = alpha_beta_server().await;
    let http = reqwest::Client::new();

    let ids = catalog_ids(&http, server.addr).await;
    assert_eq!(ids, vec!["alpha-model"]);

    let listed = json_within(
        send_within(
            http.get(format!("http://{}/admin/profiles", server.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(listed["profiles"], serde_json::json!(["alpha", "beta"]));

    let events = switch_stream_events(&http, server.addr, "beta").await;
    assert_eq!(
        events.last(),
        Some(&serde_json::json!({ "status": "ready", "profile": "beta" }))
    );

    let ids = catalog_ids(&http, server.addr).await;
    assert_eq!(ids, vec!["beta-model"]);

    let status = json_within(
        send_within(
            http.get(format!("http://{}/admin/status", server.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(status["profile"], "beta");
    assert_eq!(status["models"], serde_json::json!(["beta-model"]));
    server.shutdown().await;
}

/// The switch stream carries its stage markers in execution order -
/// loading-profile, then stopping-models, then starting-models - and ends
/// with the terminal ready event naming the new profile. No drain stage
/// exists because the gateway does not drain.
#[cfg(feature = "local")]
#[tokio::test]
async fn switch_profile_streams_stages_in_order_then_ready() {
    let (_profiles, server) = alpha_beta_server().await;
    let http = reqwest::Client::new();

    let events = switch_stream_events(&http, server.addr, "beta").await;
    assert_eq!(
        events,
        vec![
            serde_json::json!({ "stage": "loading-profile" }),
            serde_json::json!({ "stage": "stopping-models" }),
            serde_json::json!({ "stage": "starting-models" }),
            serde_json::json!({ "status": "ready", "profile": "beta" }),
        ]
    );
    server.shutdown().await;
}

/// A headless build has no local children to stop or start, so the switch
/// stream carries only the loading-profile stage before the terminal ready.
#[cfg(not(feature = "local"))]
#[tokio::test]
async fn headless_switch_streams_loading_stage_then_ready() {
    let (_profiles, server) = alpha_beta_server().await;
    let http = reqwest::Client::new();

    let events = switch_stream_events(&http, server.addr, "beta").await;
    assert_eq!(
        events,
        vec![
            serde_json::json!({ "stage": "loading-profile" }),
            serde_json::json!({ "status": "ready", "profile": "beta" }),
        ]
    );
    server.shutdown().await;
}

/// The switch's stages are hub events: a `/admin/progress` subscriber sees
/// the switch operation's leaves begin in execution order while the
/// per-request stream runs - one source of truth for both views.
#[cfg(feature = "local")]
#[tokio::test]
async fn switch_stages_are_published_on_the_progress_hub() {
    let (_profiles, server) = alpha_beta_server().await;
    let http = reqwest::Client::new();

    // Connect the hub stream before the switch so no event is missed; the
    // hub is idle at this point, so every `Begun` belongs to the switch.
    let mut progress = send_within(
        http.get(format!("http://{}/admin/progress", server.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(progress.status().as_u16(), 200);

    let addr = server.addr;
    let switch = tokio::spawn(async move {
        let http = reqwest::Client::new();
        switch_stream_events(&http, addr, "beta").await
    });

    let mut labels = Vec::new();
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + PHASE_TIMEOUT;
    while labels.len() < 3 {
        let chunk = tokio::time::timeout_at(deadline, progress.chunk())
            .await
            .expect("hub stream exceeded the phase timeout")
            .expect("hub stream read failed")
            .expect("hub stream ended");
        text.push_str(std::str::from_utf8(&chunk).expect("SSE frames are UTF-8"));
        while let Some(end) = text.find("\n\n") {
            let block: String = text.drain(..end + 2).collect();
            if let Some(data) = block.trim().strip_prefix("data: ") {
                let event: Value = serde_json::from_str(data).expect("json event");
                if event["state"].get("Begun").is_some() {
                    labels.push(event["label"].as_str().expect("label").to_owned());
                }
            }
        }
    }
    assert_eq!(
        labels,
        ["loading-profile", "stopping-models", "starting-models"]
    );
    let events = join_within(switch).await;
    assert_eq!(
        events.last(),
        Some(&serde_json::json!({ "status": "ready", "profile": "beta" }))
    );
    // The hub stream is open-ended; dropping the response disconnects so
    // graceful shutdown is not held by an idle SSE connection.
    drop(progress);
    server.shutdown().await;
}

/// A headless build reports zero local children on `/admin/status` and
/// mounts no `/v1/cache` routes.
#[cfg(not(feature = "local"))]
#[tokio::test]
async fn headless_status_reports_zero_local_children_and_no_cache_routes() {
    let (_profiles, server) = alpha_beta_server().await;
    let http = reqwest::Client::new();

    let status = json_within(
        send_within(
            http.get(format!("http://{}/admin/status", server.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(status["local_children"], serde_json::json!(0));

    let response = send_within(
        http.get(format!("http://{}/v1/cache", server.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 404);
    server.shutdown().await;
}

/// A headless build refuses a switch to a profile declaring
/// `[[local_model]]`: the stream ends with a terminal error event naming the
/// `start-local` stage, and the live profile stays intact.
#[cfg(not(feature = "local"))]
#[tokio::test]
async fn headless_switch_refuses_profile_declaring_local_models() {
    let (profiles, server) = alpha_beta_server().await;
    let http = reqwest::Client::new();

    // A profile matching the boot [server] but declaring a local model.
    fs::write(
        profiles.path().join("local-beta.toml"),
        r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[local_model]]
name = "local-q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
"#,
    )
    .unwrap();

    let events = switch_stream_events(&http, server.addr, "local-beta").await;
    assert_eq!(
        events.first(),
        Some(&serde_json::json!({ "stage": "loading-profile" }))
    );
    let terminal = events.last().expect("a terminal event");
    assert_eq!(terminal["status"], "error");
    let message = terminal["message"].as_str().expect("message");
    assert!(
        message.contains("switch profile failed at start-local"),
        "terminal event: {terminal}"
    );
    assert!(
        message.contains("lacks the `local` feature"),
        "terminal event: {terminal}"
    );

    // The live profile is untouched.
    assert_eq!(catalog_ids(&http, server.addr).await, vec!["alpha-model"]);
    server.shutdown().await;
}

/// A failed switch (missing profile) leaves the live profile fully intact:
/// same catalog, same working bearer key (LIB-009 stable credential). The
/// failure arrives as the stream's terminal error event, after only the
/// loading-profile stage - no child was stopped or started.
#[tokio::test]
async fn failed_switch_leaves_live_profile_intact() {
    let (_profiles, server) = alpha_beta_server().await;
    let http = reqwest::Client::new();

    // Switch to a profile that does not exist: a terminal error event, no
    // state change.
    let events = switch_stream_events(&http, server.addr, "ghost").await;
    assert_eq!(
        events.first(),
        Some(&serde_json::json!({ "stage": "loading-profile" }))
    );
    let terminal = events.last().expect("a terminal event");
    assert_eq!(terminal["status"], "error");
    assert_eq!(
        terminal["message"].as_str().expect("message"),
        "profile not found: ghost"
    );
    assert_eq!(events.len(), 2, "no stopping or starting stage: {events:?}");

    // The original catalog and bearer key still work unchanged.
    assert_eq!(catalog_ids(&http, server.addr).await, vec!["alpha-model"]);
    server.shutdown().await;
}

/// A failed switch fails its phase leaf on the hub: a `/admin/progress`
/// subscriber sees `loading-profile` end with `Finished { ok: false }`
/// rather than staying live until the switch's tree detaches.
#[tokio::test]
async fn a_failed_switch_fails_its_phase_leaf_on_the_hub() {
    let (_profiles, server) = alpha_beta_server().await;
    let http = reqwest::Client::new();

    // Connect the hub stream before the switch so no event is missed; the
    // hub is idle at this point, so every event belongs to the switch.
    let mut progress = send_within(
        http.get(format!("http://{}/admin/progress", server.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(progress.status().as_u16(), 200);

    let addr = server.addr;
    let switch = tokio::spawn(async move {
        let http = reqwest::Client::new();
        switch_stream_events(&http, addr, "ghost").await
    });

    let mut terminal = None;
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + PHASE_TIMEOUT;
    while terminal.is_none() {
        let chunk = tokio::time::timeout_at(deadline, progress.chunk())
            .await
            .expect("hub stream exceeded the phase timeout")
            .expect("hub stream read failed")
            .expect("hub stream ended");
        text.push_str(std::str::from_utf8(&chunk).expect("SSE frames are UTF-8"));
        while let Some(end) = text.find("\n\n") {
            let block: String = text.drain(..end + 2).collect();
            if let Some(data) = block.trim().strip_prefix("data: ") {
                let event: Value = serde_json::from_str(data).expect("json event");
                if event["label"] == "loading-profile"
                    && let Some(finished) = event["state"].get("Finished")
                {
                    terminal = Some(finished["ok"].as_bool().expect("ok flag"));
                }
            }
        }
    }
    assert_eq!(
        terminal,
        Some(false),
        "the missing profile fails the loading-profile leaf"
    );
    let events = join_within(switch).await;
    assert_eq!(events.last().expect("a terminal event")["status"], "error");
    // The hub stream is open-ended; dropping the response disconnects so
    // graceful shutdown is not held by an idle SSE connection.
    drop(progress);
    server.shutdown().await;
}

/// The boot file owns `[server]`: a profile whose merged `[server]` differs
/// is rejected at switch time - a terminal error event on the stream -
/// leaving the live profile fully intact.
#[tokio::test]
async fn switch_profile_with_mismatched_server_section_fails() {
    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();

    let profile_toml = |model: &str, key: &str| {
        format!(
            r#"
[server]
bind = "127.0.0.1:0"
api_key = "{key}"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "{model}"
description = "{model} catalog entry"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
        )
    };
    fs::write(
        profiles.path().join("alpha.toml"),
        profile_toml("alpha-model", "test-token"),
    )
    .unwrap();
    fs::write(
        profiles.path().join("beta.toml"),
        profile_toml("beta-model", "other-token"),
    )
    .unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;
    let http = reqwest::Client::new();

    let events = switch_stream_events(&http, server.addr, "beta").await;
    let terminal = events.last().expect("a terminal event");
    assert_eq!(terminal["status"], "error");
    assert!(
        terminal["message"]
            .as_str()
            .expect("message")
            .contains("switch profile failed at server-mismatch"),
        "terminal event: {terminal}"
    );

    // The live profile is untouched: same catalog, same working bearer key.
    assert_eq!(catalog_ids(&http, server.addr).await, vec!["alpha-model"]);
    server.shutdown().await;
}

/// The boot config owns `[workshop]`: a profile whose merged `[workshop]`
/// differs is rejected at switch time - a terminal error event on the
/// stream - leaving the live profile fully intact. The hosted workshop is
/// started once at boot, so a switch can never move or reconfigure it.
#[tokio::test]
async fn switch_profile_with_mismatched_workshop_section_fails() {
    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();

    let profile_toml = |model: &str, workshop_bind: &str| {
        format!(
            r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[workshop]
bind = "{workshop_bind}"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "{model}"
description = "{model} catalog entry"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
        )
    };
    fs::write(
        profiles.path().join("alpha.toml"),
        profile_toml("alpha-model", "127.0.0.1:7910"),
    )
    .unwrap();
    fs::write(
        profiles.path().join("beta.toml"),
        profile_toml("beta-model", "127.0.0.1:7911"),
    )
    .unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;
    let http = reqwest::Client::new();

    let events = switch_stream_events(&http, server.addr, "beta").await;
    let terminal = events.last().expect("a terminal event");
    assert_eq!(terminal["status"], "error");
    assert!(
        terminal["message"]
            .as_str()
            .expect("message")
            .contains("switch profile failed at workshop-mismatch"),
        "terminal event: {terminal}"
    );

    // The live profile is untouched: same catalog, same working bearer key.
    assert_eq!(catalog_ids(&http, server.addr).await, vec!["alpha-model"]);
    server.shutdown().await;
}

/// A catalog with two remote models and two local models bound to one
/// 24 GiB local dominion, over-booked in total (14 + 14 > 24). `{backend}`
/// is substituted with the fake backend address.
fn allowlist_catalog(backend: std::net::SocketAddr) -> String {
    format!(
        r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "remote-a"
description = "remote a catalog entry"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "remote-b"
description = "remote b catalog entry"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[local_model]]
name = "local-a"
description = "local a catalog entry"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14

[[local_model]]
name = "local-b"
description = "local b catalog entry"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14
"#
    )
}

/// Writes `catalog.toml` plus one profile per `(name, allowlist)` pair into a
/// tempdir layout: `<tmp>/catalog.toml`, `<tmp>/profiles/<name>.toml`.
fn allowlist_fixture(catalog: &str, profiles: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("catalog.toml"), catalog).unwrap();
    let dir = tmp.path().join("profiles");
    fs::create_dir(&dir).unwrap();
    for (name, allowlist) in profiles {
        fs::write(
            dir.join(format!("{name}.toml")),
            format!("include = [\"../catalog.toml\"]\nmodels = {allowlist}\n"),
        )
        .unwrap();
    }
    tmp
}

/// A profile's `models` allowlist selects the loaded set: the catalog shows
/// exactly the selection, `/admin/status` reports it, and a switch swaps both.
#[tokio::test]
async fn switch_profile_changes_the_loaded_set() {
    let backend = fake_backend().await;
    let tmp = allowlist_fixture(
        &allowlist_catalog(backend),
        &[("alpha", "[\"remote-a\"]"), ("beta", "[\"remote-b\"]")],
    );
    let dir = tmp.path().join("profiles");

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(&dir, &alpha).unwrap();
    let context = ProfilesContext::new(Some(dir.clone()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;
    let http = reqwest::Client::new();

    // The loaded set is the selection, not the full catalog.
    let ids = catalog_ids(&http, server.addr).await;
    assert_eq!(ids, vec!["remote-a"]);
    let status = json_within(
        send_within(
            http.get(format!("http://{}/admin/status", server.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(status["profile"], "alpha");
    assert_eq!(status["model_allowlist"], serde_json::json!(["remote-a"]));

    let events = switch_stream_events(&http, server.addr, "beta").await;
    assert_eq!(
        events.last(),
        Some(&serde_json::json!({ "status": "ready", "profile": "beta" }))
    );

    let ids = catalog_ids(&http, server.addr).await;
    assert_eq!(ids, vec!["remote-b"]);
    let status = json_within(
        send_within(
            http.get(format!("http://{}/admin/status", server.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(status["profile"], "beta");
    assert_eq!(status["model_allowlist"], serde_json::json!(["remote-b"]));
    server.shutdown().await;
}

/// The VRAM co-residency check runs on the profile's loaded set at switch: a
/// selection that over-books the dominion is rejected before any child starts,
/// leaving the live profile intact.
#[tokio::test]
async fn switch_profile_runs_vram_check_on_the_new_loaded_set() {
    let backend = fake_backend().await;
    let tmp = allowlist_fixture(
        &allowlist_catalog(backend),
        &[
            ("alpha", "[\"remote-a\"]"),
            ("gamma", "[\"local-a\", \"local-b\"]"),
        ],
    );
    let dir = tmp.path().join("profiles");

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(&dir, &alpha).unwrap();
    let context = ProfilesContext::new(Some(dir.clone()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;
    let http = reqwest::Client::new();

    // gamma selects both local models, over-booking gpu0 (14 + 14 > 24): the
    // switch fails at config load - after the loading-profile stage but
    // before any stopping or starting stage, so no llama-server child was
    // touched - and the stream ends with a terminal error event.
    let events = switch_stream_events(&http, server.addr, "gamma").await;
    assert_eq!(
        events.first(),
        Some(&serde_json::json!({ "stage": "loading-profile" }))
    );
    let terminal = events.last().expect("a terminal event");
    assert_eq!(terminal["status"], "error");
    assert!(
        terminal["message"]
            .as_str()
            .expect("message")
            .contains("switch profile failed at load-profile"),
        "terminal event: {terminal}"
    );
    assert_eq!(events.len(), 2, "no stopping or starting stage: {events:?}");

    // The live profile is untouched.
    assert_eq!(catalog_ids(&http, server.addr).await, vec!["remote-a"]);
    let status = json_within(
        send_within(
            http.get(format!("http://{}/admin/status", server.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(status["profile"], "alpha");
    assert_eq!(status["model_allowlist"], serde_json::json!(["remote-a"]));
    server.shutdown().await;
}

/// A traversal profile name is rejected before touching the filesystem.
#[tokio::test]
async fn switch_profile_rejects_traversal_name() {
    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();
    std::fs::write(
        profiles.path().join("alpha.toml"),
        format!(
            "[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n[[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://{backend}\"\napi_key = \"\"\n\n[[model]]\nname = \"alpha-model\"\ndescription = \"alpha\"\ncontext = 8192\nupstream = \"backend-model\"\nendpoints = [\"fake\"]\n"
        ),
    )
    .unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/admin/switch-profile", server.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": "../escape" })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 400);
    server.shutdown().await;
}
