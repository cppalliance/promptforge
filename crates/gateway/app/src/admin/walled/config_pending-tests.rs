use gateway_config::{Config, ProfileSelection, profile_state_path, shadow_path, write_shadow};

use super::*;
use crate::test_support::{AdminPaths, serve_with_paths};

const PROFILE_CONFIG: &str = r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[profile]]
name = "alpha"
models = []

[[profile]]
name = "beta"
models = []
"#;

#[test]
fn dirty_reply_lists_the_config_shadow_and_never_active_profile() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config = temp.path().join("gateway.toml");
    std::fs::write(
        &config,
        "config-version = 0\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n",
    )
    .expect("write config");
    let state = profile_state_path(&config);
    std::fs::write(&state, "active_profile = \"alpha\"\n").expect("write state");
    write_shadow(
        &config,
        "config-version = 0\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"changed\"\n",
    )
    .expect("write config shadow");
    // A leftover from before selection stopped staging: never reported.
    write_shadow(&state, "active_profile = \"beta\"\n").expect("write stale state shadow");

    let reply = dirty_reply(&config).expect("dirty reply");

    assert!(reply.dirty);
    assert_eq!(reply.pending_files, ["gateway.toml"]);
    assert_eq!(reply.changed_sections, ["server"]);
    assert!(shadow_path(&config).is_file());
}

/// A state file that exists but cannot be read is a server fault whose
/// message names the file, so the 500 body stands on its own.
#[test]
fn an_unreadable_state_file_names_itself_in_the_error() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config = temp.path().join("gateway.toml");
    let state = profile_state_path(&config);
    // A directory in the file's place reads as an error that is not
    // `NotFound` on every platform.
    std::fs::create_dir(&state).expect("occupy the state path");

    let error = persisted_selection(&config).expect_err("a directory is not a state file");

    let GatewayError::PendingConfig(message) = error else {
        panic!("a read failure is a pending-config fault: {error:?}");
    };
    assert!(
        message.contains(&state.display().to_string()),
        "the message names the file: {message}"
    );
}

/// Serves `PROFILE_CONFIG` from `temp` with `running` as the live
/// profile (a command-line override), leaving the state file to the test.
async fn serve_profiles(temp: &tempfile::TempDir, running: &str) -> std::net::SocketAddr {
    let config_path = temp.path().join("gateway.toml");
    std::fs::write(&config_path, PROFILE_CONFIG).expect("write config");
    let config = Config::load(&config_path, &ProfileSelection::new(Some(running), None))
        .expect("load command-line override");
    serve_with_paths(
        config,
        AdminPaths {
            fixture_dir: temp.path().to_path_buf(),
            active: running.to_owned(),
            config_path,
        },
    )
    .await
}

async fn pending_active_profile(addr: std::net::SocketAddr) -> serde_json::Value {
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/admin/config-pending"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("pending request sends");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let mut body: serde_json::Value = response.json().await.expect("pending body is JSON");
    assert!(body["boot"].is_null(), "the envelope keeps its shape");
    body["profile"]["active_profile"].take()
}

#[tokio::test]
async fn pending_view_reports_null_when_no_selection_is_persisted() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let addr = serve_profiles(&temp, "beta").await;

    assert!(
        pending_active_profile(addr).await.is_null(),
        "a running command-line override is not a persisted selection"
    );
}

#[tokio::test]
async fn pending_view_reports_the_state_files_name_even_when_stale() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let addr = serve_profiles(&temp, "beta").await;
    let state = profile_state_path(&temp.path().join("gateway.toml"));

    std::fs::write(&state, "active_profile = \"alpha\"\n").expect("write state");
    assert_eq!(pending_active_profile(addr).await, "alpha");

    std::fs::write(&state, "active_profile = \"retired\"\n").expect("write stale state");
    assert_eq!(
        pending_active_profile(addr).await,
        "retired",
        "the raw persisted name is reported even when the config no longer defines it"
    );
}
