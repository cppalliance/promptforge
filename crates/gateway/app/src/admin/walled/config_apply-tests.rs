use std::net::SocketAddr;
use std::sync::Arc;

use gateway_config::{Config, ProfileSelection, profile_state_path, shadow_path, write_shadow};

use crate::AppState;
use crate::commands::apply::{ApplyPlan, capture_apply};
use crate::test_support::{AdminPaths, app_state, serve_state};

#[path = "config_apply-tests-in-flight.rs"]
mod in_flight;

const CONFIG: &str = r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "alpha-model"
description = "alpha"
context = 1024
upstream = "alpha"
endpoints = ["fake"]

[[model]]
name = "beta-model"
description = "beta"
context = 1024
upstream = "beta"
endpoints = ["fake"]

[[profile]]
name = "alpha"
models = []

[[profile]]
name = "beta"
models = []
"#;

/// A third remote model appended to `CONFIG`: the shape of the one
/// change an apply reloads live.
const GAMMA_MODEL: &str = "\n[[model]]\nname = \"gamma-model\"\ndescription = \"gamma\"\n\
                           context = 1024\nupstream = \"gamma\"\nendpoints = [\"fake\"]\n";

fn fixture() -> (tempfile::TempDir, Config, AdminPaths) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("gateway.toml");
    std::fs::write(&config_path, CONFIG).expect("write config");
    std::fs::write(
        profile_state_path(&config_path),
        "active_profile = \"alpha\"\n",
    )
    .expect("write state");
    let config = Config::load(&config_path, &ProfileSelection::default()).expect("load config");
    let paths = AdminPaths {
        fixture_dir: temp.path().to_path_buf(),
        active: "alpha".to_owned(),
        config_path,
    };
    (temp, config, paths)
}

/// Serves the fixture with the production queue worker running, so an
/// apply's `ApplyConfig` command actually drains; the state comes back
/// for tests that read the queue or the live table.
async fn serve_fixture(config: Config, paths: AdminPaths) -> (SocketAddr, AppState) {
    let state = app_state(config, Some(paths));
    let _worker = state.commands.spawn_worker(&state).expect("worker spawns");
    let addr = serve_state(state.clone()).await;
    (addr, state)
}

/// Stages `CONFIG` plus `GAMMA_MODEL` as the pending config.
fn stage_gamma(config_path: &std::path::Path) {
    write_shadow(config_path, &format!("{CONFIG}{GAMMA_MODEL}")).expect("stage config shadow");
}

async fn post(addr: SocketAddr, route: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/{route}"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("post sends")
}

async fn get_json(addr: SocketAddr, route: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("http://{addr}/{route}"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("get sends")
        .json()
        .await
        .expect("json body")
}

/// Saves the live config with `edit` applied through the real save
/// route, so the shadow is exactly what the UI would write: the running
/// `active_profile` that `GET /admin/config` reports is not a
/// configuration key and never goes back in a save.
async fn save_edited(
    addr: SocketAddr,
    edit: impl FnOnce(&mut serde_json::Value),
) -> reqwest::Response {
    let mut body = get_json(addr, "admin/config").await;
    body.as_object_mut()
        .expect("the config is an object")
        .remove("active_profile");
    edit(&mut body);
    reqwest::Client::new()
        .put(format!("http://{addr}/admin/config"))
        .bearer_auth("test-token")
        .json(&body)
        .send()
        .await
        .expect("save sends")
}

/// The live profile name, as `GET /admin/status` would report it.
async fn live_profile(state: &AppState) -> Option<String> {
    state.live.read().await.profile_name.clone()
}

/// Whether the live routing table resolves `name`.
async fn routes(state: &AppState, name: &str) -> bool {
    state.live.read().await.routing.model(name).is_ok()
}

/// A child-free local runtime holding one model named `name`, standing
/// in for a running `llama-server` the apply must keep routing.
#[cfg(feature = "test-fixtures")]
fn running_local(name: &str) -> crate::local::LocalRuntime {
    let config = Config::from_toml_str(&format!(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"local\"\nprotocol = \"openai\"\n\
         base_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
         [[model]]\nname = \"{name}\"\ndescription = \"running child\"\n\
         context = 4096\nupstream = \"{name}\"\nendpoints = [\"local\"]\n"
    ))
    .expect("local fixture config parses");
    let routing =
        crate::routing::Routing::from_config(&config).expect("local fixture routing builds");
    crate::local::LocalRuntime::from_test_models(routing.models().to_vec())
}

/// The reload an apply performs: a new `[[model]]` enters the live
/// routing table, the running local child keeps its entry, the shadow
/// promotes, and the reply says so without a restart.
#[cfg(feature = "test-fixtures")]
#[tokio::test]
async fn apply_with_a_new_model_swaps_the_routing_live_and_promotes_the_shadow() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let (addr, state) = serve_fixture(config, paths).await;
    {
        let mut live = state.live.write().await;
        live.local = running_local("alpha-local");
        let routing = Arc::clone(&live.routing);
        live.routing = Arc::new(
            routing
                .as_ref()
                .clone()
                .merge(live.local.models().iter().cloned())
                .expect("the running child routes"),
        );
    }
    stage_gamma(&config_path);

    let response = post(addr, "admin/config-apply").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = response.json().await.expect("apply body");
    assert_eq!(reply["reloaded"], true);
    assert_eq!(reply["restart_required"], false);
    assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));
    assert!(routes(&state, "gamma-model").await, "the new model routes");
    assert!(
        routes(&state, "alpha-local").await,
        "the running local child keeps its routing entry"
    );
    assert_eq!(
        state.live.read().await.local.child_count(),
        1,
        "the local runtime is untouched"
    );
    assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
    assert!(!shadow_path(&config_path).exists(), "the shadow promoted");
    assert!(
        std::fs::read_to_string(&config_path)
            .expect("read applied config")
            .contains("gamma-model"),
        "the real file contains the applied change"
    );
    let served = get_json(addr, "admin/config").await;
    assert_eq!(served["model"][2]["name"], "gamma-model");
    assert!(
        !state.hub.current().busy,
        "the settled apply released its activity: {:?}",
        state.hub.current()
    );
}

/// A persisted selection that differs from the running profile (a
/// switch that persisted a new name and awaits a restart) never reaches
/// the live document: the applied config has no selection, the
/// running profile is unchanged, and `GET /admin/config` does not report
/// the persisted name as the running one.
#[tokio::test]
async fn apply_publishes_the_document_without_the_persisted_selection() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let (addr, state) = serve_fixture(config, paths).await;
    std::fs::write(
        profile_state_path(&config_path),
        "active_profile = \"beta\"\n",
    )
    .expect("persist a selection awaiting restart");
    stage_gamma(&config_path);

    let response = post(addr, "admin/config-apply").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(routes(&state, "gamma-model").await);
    assert!(
        state.live.read().await.config.active_profile().is_none(),
        "the live document omits the selection"
    );
    assert_eq!(live_profile(&state).await.as_deref(), Some("alpha"));
    let served = get_json(addr, "admin/config").await;
    assert!(
        served.get("active_profile").is_none(),
        "the persisted name is not reported as running: {served}"
    );
}

/// Each boot-read section flags a restart; the live-reloadable ones do
/// not. Every case stages a valid config differing from the real one in
/// exactly that section.
#[test]
fn capture_apply_flags_restart_for_boot_read_sections_only() {
    let cases: [(&str, &str, bool); 7] = [
        (
            "profile",
            "\n[[profile]]\nname = \"gamma\"\nmodels = []\n",
            true,
        ),
        (
            "local_model",
            "\n[[local_model]]\nname = \"gamma\"\ndescription = \"g\"\n\
             source = \"/models/gamma.gguf\"\ncontext = 4096\n",
            true,
        ),
        (
            "stt_model",
            "\n[[stt_model]]\nname = \"speech\"\nrole = \"interim\"\n\
             source = \"/speech.bin\"\nvram_gb = 1.0\n",
            true,
        ),
        (
            "stt",
            "\n[stt]\nwindow_seconds = 8\ninterval_ms = 250\n",
            true,
        ),
        ("model", GAMMA_MODEL, false),
        (
            "endpoint",
            "\n[[endpoint]]\nid = \"other\"\nprotocol = \"openai\"\n\
             base_url = \"http://127.0.0.1:10\"\napi_key = \"\"\n",
            false,
        ),
        (
            "tools",
            "\n[tools.web_search]\nprovider = \"brave\"\napi_key = \"k\"\n",
            false,
        ),
    ];
    for (section, addition, expected) in cases {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let config_path = temp.path().join("gateway.toml");
        std::fs::write(&config_path, CONFIG).expect("write config");
        write_shadow(&config_path, &format!("{CONFIG}{addition}")).expect("stage shadow");

        let plan = capture_apply(&config_path).expect("the pending config captures");

        let ApplyPlan::Reload(snapshot) = plan else {
            panic!("a config shadow always reloads: {section}");
        };
        assert_eq!(
            snapshot.restart_required, expected,
            "restart_required for a {section} change"
        );
        assert_eq!(snapshot.applied_names(), ["gateway.toml"]);
    }
}

#[tokio::test]
async fn invalid_pending_config_is_never_promoted() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let original_config = std::fs::read_to_string(&config_path).expect("read config");
    write_shadow(&config_path, "not valid TOML [[[").expect("stage tampered shadow");
    let (addr, state) = serve_fixture(config, paths).await;

    let response = post(addr, "admin/config-apply").await;

    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        std::fs::read_to_string(&config_path).expect("re-read config"),
        original_config
    );
    assert!(
        shadow_path(&config_path).is_file(),
        "the rejected shadow remains available for correction or revert"
    );
    assert!(
        !state.hub.current().busy,
        "the parse failure replies before any command exists"
    );
    assert!(state.commands.active_command().is_none());
    assert!(state.commands.pending_commands().is_empty());
}

#[tokio::test]
async fn env_only_apply_requires_restart_without_a_command() {
    let (_temp, config, paths) = fixture();
    let env_path = paths.config_path.with_extension("env");
    write_shadow(&env_path, "HF_TOKEN=pending\n").expect("stage env shadow");
    let (addr, state) = serve_fixture(config, paths).await;

    let response = post(addr, "admin/config-apply").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = response.json().await.expect("apply body");
    assert_eq!(reply["applied"], serde_json::json!(["gateway.env"]));
    assert_eq!(reply["reloaded"], false);
    assert_eq!(reply["restart_required"], true);
    assert_eq!(
        std::fs::read_to_string(&env_path).expect("read promoted env"),
        "HF_TOKEN=pending\n"
    );
    assert!(
        !shadow_path(&env_path).exists(),
        "the promoted shadow is retired"
    );
    assert!(
        !state.hub.current().busy && state.commands.active_command().is_none(),
        "the no-reload path promotes inline without a command"
    );
}

#[tokio::test]
async fn server_key_change_waits_for_restart() {
    let (_temp, config, paths) = fixture();
    let (addr, _state) = serve_fixture(config, paths).await;
    let http = reqwest::Client::new();
    let save = save_edited(addr, |body| {
        body["server"]["api_key"] = serde_json::json!("next-token");
    })
    .await;
    assert_eq!(save.status(), reqwest::StatusCode::OK);

    let response = post(addr, "admin/config-apply").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = response.json().await.expect("apply body");
    assert_eq!(reply["restart_required"], true);
    assert_eq!(
        http.get(format!("http://{addr}/v1/models"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("old token request sends")
            .status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(
        http.get(format!("http://{addr}/v1/models"))
            .bearer_auth("next-token")
            .send()
            .await
            .expect("new token request sends")
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
}

/// The speech pipeline is configured once at boot: an `[stt]` change
/// promotes and reloads the document but reports a restart.
#[tokio::test]
async fn stt_pipeline_change_promotes_and_requires_restart() {
    let (_temp, config, paths) = fixture();
    write_shadow(
        &paths.config_path,
        &format!(
            "{CONFIG}\n[stt]\nwindow_seconds = 8\ninterval_ms = 250\n\
             vocabulary = [\"WG21\"]\n"
        ),
    )
    .expect("stage STT-only shadow");
    let (addr, _state) = serve_fixture(config, paths).await;
    let dirty = get_json(addr, "admin/config-dirty").await;
    assert_eq!(dirty["changed_sections"], serde_json::json!(["stt"]));

    let response = post(addr, "admin/config-apply").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = response.json().await.expect("apply body");
    assert_eq!(reply["reloaded"], true);
    assert_eq!(reply["restart_required"], true);
    let applied = get_json(addr, "admin/config").await;
    assert_eq!(applied["stt"]["window_seconds"], 8);
    assert_eq!(applied["stt"]["interval_ms"], 250);
    assert_eq!(applied["stt"]["vocabulary"], serde_json::json!(["WG21"]));
}

#[tokio::test]
async fn revert_removes_all_shadows_without_touching_real_files() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let env_path = config_path.with_extension("env");
    let original_config = std::fs::read_to_string(&config_path).expect("read config");
    stage_gamma(&config_path);
    write_shadow(&env_path, "HF_TOKEN=pending\n").expect("stage env");
    let (addr, _state) = serve_fixture(config, paths).await;

    let response = post(addr, "admin/config-revert").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = response.json().await.expect("revert body");
    assert_eq!(
        reply["reverted"],
        serde_json::json!(["gateway.env.next", "gateway.toml.next"])
    );
    assert_eq!(
        std::fs::read_to_string(&config_path).expect("re-read config"),
        original_config
    );
    assert!(!env_path.exists());
}
