//! The switch-profile route: persistence, restart reporting, and refusals that change nothing.
//! `POST /admin/switch-profile` persists the selection and reports
//! whether a restart is needed; it never touches the live state.

use std::sync::Arc;

use gateway_config::{Config, ProfileSelection, profile_state_path};

use crate::AppState;
use crate::test_support::{AdminPaths, app_state, serve_state};

const TWO_PROFILES: &str = "config-version = 0\n\
     [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
     [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\n\
     base_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
     [[model]]\nname = \"m\"\ndescription = \"d\"\n\
     context = 8192\nupstream = \"u\"\nendpoints = [\"e\"]\n\
     [[profile]]\nname = \"alpha\"\nmodels = []\n\
     [[profile]]\nname = \"beta\"\nmodels = []\n";

/// Writes the two-profile catalog into `temp` with no state file and
/// builds the state `alpha` runs under, as a `--profile alpha` boot
/// would leave it. Returns the state and the state file's path.
fn running_alpha(temp: &tempfile::TempDir) -> (AppState, std::path::PathBuf) {
    let config_path = temp.path().join("gateway.toml");
    std::fs::write(&config_path, TWO_PROFILES).expect("write catalog");
    let config = Config::load(&config_path, &ProfileSelection::new(Some("alpha"), None))
        .expect("alpha loads");
    let state = app_state(
        config,
        Some(AdminPaths {
            fixture_dir: temp.path().to_path_buf(),
            active: "alpha".to_owned(),
            config_path: config_path.clone(),
        }),
    );
    (state, profile_state_path(&config_path))
}

/// What a switch must leave alone: the routing table, the running
/// profile, and the local children.
struct LiveSnapshot {
    routing: Arc<crate::routing::Routing>,
    profile_name: Option<String>,
    #[cfg(feature = "local")]
    local_models: Vec<String>,
}

async fn snapshot(state: &AppState) -> LiveSnapshot {
    let live = state.live.read().await;
    LiveSnapshot {
        routing: Arc::clone(&live.routing),
        profile_name: live.profile_name.clone(),
        #[cfg(feature = "local")]
        local_models: live
            .local
            .models()
            .iter()
            .map(|model| model.name.clone())
            .collect(),
    }
}

async fn assert_live_unchanged(state: &AppState, before: &LiveSnapshot) {
    let after = snapshot(state).await;
    assert!(
        Arc::ptr_eq(&before.routing, &after.routing),
        "a switch never swaps the routing table"
    );
    assert_eq!(
        before.profile_name, after.profile_name,
        "a switch never changes the running profile"
    );
    #[cfg(feature = "local")]
    assert_eq!(
        before.local_models, after.local_models,
        "a switch never starts or stops local children"
    );
}

async fn switch(addr: std::net::SocketAddr, body: serde_json::Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/admin/switch-profile"))
        .bearer_auth("test-token")
        .json(&body)
        .send()
        .await
        .expect("the switch request sends")
}

async fn get_json(addr: std::net::SocketAddr, path: &str) -> serde_json::Value {
    let response = reqwest::Client::new()
        .get(format!("http://{addr}{path}"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("the request sends");
    assert_eq!(response.status(), reqwest::StatusCode::OK, "{path}");
    response.json().await.expect("the body is JSON")
}

#[tokio::test]
async fn switching_to_a_different_defined_profile_persists_it_and_requires_a_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (state, state_path) = running_alpha(&temp);
    let before = snapshot(&state).await;
    let addr = serve_state(state.clone()).await;

    let response = switch(addr, serde_json::json!({ "name": "beta" })).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("the reply is JSON");
    assert_eq!(
        body,
        serde_json::json!({ "profile": "beta", "restart_required": true })
    );
    assert_eq!(
        std::fs::read_to_string(&state_path).expect("the state file is written"),
        "active_profile = \"beta\"\n"
    );
    assert_live_unchanged(&state, &before).await;
    assert_eq!(
        get_json(addr, "/admin/config-pending").await["profile"]["active_profile"],
        "beta",
        "the pending view reports the persisted selection"
    );
    assert_eq!(
        get_json(addr, "/admin/status").await["profile"],
        "alpha",
        "the status readout reports the running profile"
    );
}

#[tokio::test]
async fn switching_to_the_running_profile_persists_it_without_a_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (state, state_path) = running_alpha(&temp);
    let before = snapshot(&state).await;
    let addr = serve_state(state.clone()).await;

    let response = switch(addr, serde_json::json!({ "name": "alpha" })).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("the reply is JSON");
    assert_eq!(
        body,
        serde_json::json!({ "profile": "alpha", "restart_required": false })
    );
    assert_eq!(
        std::fs::read_to_string(&state_path).expect("the state file is written"),
        "active_profile = \"alpha\"\n",
        "an ephemeral --profile boot becomes persisted by selecting it"
    );
    assert_live_unchanged(&state, &before).await;
}

#[tokio::test]
async fn switching_to_an_undefined_profile_names_the_defined_ones_and_writes_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (state, state_path) = running_alpha(&temp);
    let before = snapshot(&state).await;
    let addr = serve_state(state.clone()).await;

    let response = switch(addr, serde_json::json!({ "name": "ghost" })).await;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let body: serde_json::Value = response.json().await.expect("the error is JSON");
    assert_eq!(body["error"]["code"], "profile_not_found");
    let message = body["error"]["message"]
        .as_str()
        .expect("the message is a string");
    assert!(
        message.contains("ghost"),
        "names the refused profile: {message}"
    );
    assert!(
        message.contains("alpha, beta"),
        "names the defined profiles: {message}"
    );
    assert!(!state_path.exists(), "a refused switch writes no state");
    assert_live_unchanged(&state, &before).await;
    assert!(
        get_json(addr, "/admin/config-pending").await["profile"]["active_profile"].is_null(),
        "nothing is persisted"
    );
}

#[tokio::test]
async fn a_malformed_name_is_refused_before_anything_is_written() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (state, state_path) = running_alpha(&temp);
    let addr = serve_state(state).await;

    let response = switch(addr, serde_json::json!({ "name": "../escape" })).await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.expect("the error is JSON");
    assert_eq!(body["error"]["code"], "switch_failed");
    assert!(!state_path.exists(), "a malformed name writes no state");
}

#[tokio::test]
async fn switching_to_null_deletes_the_state_file_and_requires_a_restart_when_a_profile_runs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (state, state_path) = running_alpha(&temp);
    std::fs::write(&state_path, "active_profile = \"alpha\"\n").expect("write state");
    let before = snapshot(&state).await;
    let addr = serve_state(state.clone()).await;

    let response = switch(addr, serde_json::json!({ "name": null })).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("the reply is JSON");
    assert_eq!(
        body,
        serde_json::json!({ "profile": null, "restart_required": true })
    );
    assert!(
        !state_path.exists(),
        "an absent state file is the persisted form of no profile"
    );
    assert_live_unchanged(&state, &before).await;
    assert!(
        get_json(addr, "/admin/config-pending").await["profile"]["active_profile"].is_null(),
        "the pending view reports no persisted selection"
    );
    assert_eq!(
        get_json(addr, "/admin/status").await["profile"],
        "alpha",
        "the running profile is untouched"
    );
}

#[tokio::test]
async fn switching_to_null_with_no_running_profile_needs_no_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (state, state_path) = running_alpha(&temp);
    state.live.write().await.profile_name = None;
    let before = snapshot(&state).await;
    let addr = serve_state(state.clone()).await;

    let response = switch(addr, serde_json::json!({ "name": null })).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("the reply is JSON");
    assert_eq!(
        body,
        serde_json::json!({ "profile": null, "restart_required": false })
    );
    assert!(!state_path.exists(), "no state file was ever written");
    assert_live_unchanged(&state, &before).await;
}
