//! Admin profile routes: switch, list, status, and failure isolation.

use std::fs;

use promptforge_gateway::{Config, Gateway, ProfileName, ProfilesContext};
use serde_json::Value;

use crate::support::{
    TestServer, catalog_ids, fake_backend, json_within, parse_sse, send_within, text_within,
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
