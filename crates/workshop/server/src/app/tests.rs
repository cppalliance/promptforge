//! App state tests: boot-time workspace reopening, auth headers, defaults, and route refusals.

use super::*;

use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::fixtures::{body_bytes, config_for, spawn_gateway, state_for};
use workshop_gateway::GatewayClient;

/// The `last-workspace` pointer file inside `state_dir`.
fn pointer_path(state_dir: &std::path::Path) -> std::path::PathBuf {
    state_dir.join("last-workspace")
}

/// Builds state anchored at `state_dir`, reopens the last workspace the
/// way boot does, and returns the state with whether anything reopened.
async fn booted_state(state_dir: &std::path::Path) -> (AppState, bool) {
    let config = config_for("http://127.0.0.1:1", state_dir);
    let gateway = ResolvedGateway::from_config(&config.gateway);
    let state = state_with_gateway(&config, &gateway).expect("boot never fails on the pointer");
    let reopened = state.reopen_last_workspace().await;
    (state, reopened)
}

/// `GET /workspace/file/current` through the full router, parsed.
async fn current_file(state: AppState) -> serde_json::Value {
    use tower::ServiceExt as _;

    let request = axum::http::Request::builder()
        .uri("/workspace/file/current")
        .body(axum::body::Body::empty())
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON")
}

#[tokio::test]
async fn boot_reopens_the_workspace_the_pointer_names() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let root = tempfile::TempDir::new().expect("tempdir");
    let file = home.path().join("mine.pfwork");
    // Author the file in a previous "run", then let go of it.
    let author = Workspace::new();
    let granted = author.grant(root.path()).expect("grant the root");
    author.save_as(&file).await.expect("save as creates");
    author.close_backing_for_test().await;
    std::fs::write(
        pointer_path(state_dir.path()),
        file.to_string_lossy().as_bytes(),
    )
    .expect("the pointer writes");

    let (state, reopened) = booted_state(state_dir.path()).await;
    assert!(reopened, "boot follows the pointer");

    let current = current_file(state).await;
    assert_eq!(current["path"], serde_json::json!(file));
    assert_eq!(current["name"], "mine");
    assert_eq!(
        current["grants"],
        serde_json::json!([{ "path": granted, "exists": true }]),
        "the file's grants are live before the listener serves"
    );
}

#[tokio::test]
async fn boot_with_corrupt_pointer_bytes_starts_ephemeral() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(pointer_path(state_dir.path()), [0xff, 0xfe, 0x00, 0xc3])
        .expect("the corrupt pointer writes");

    let (state, reopened) = booted_state(state_dir.path()).await;
    assert!(!reopened, "garbage names no workspace");

    let current = current_file(state).await;
    assert_eq!(current["path"], serde_json::Value::Null);
    assert_eq!(current["grants"], serde_json::json!([]));
}

#[tokio::test]
async fn boot_with_a_pointer_to_a_vanished_file_starts_ephemeral() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let gone = home.path().join("gone.pfwork");
    std::fs::write(
        pointer_path(state_dir.path()),
        gone.to_string_lossy().as_bytes(),
    )
    .expect("the pointer writes");

    let (state, reopened) = booted_state(state_dir.path()).await;
    assert!(!reopened, "a vanished target cannot reopen");

    let current = current_file(state).await;
    assert_eq!(current["path"], serde_json::Value::Null);
    assert!(!gone.exists(), "boot never creates the pointed file");
}

#[tokio::test]
async fn boot_with_a_pointer_to_an_alien_database_starts_ephemeral() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let alien = home.path().join("alien.pfwork");
    workshop_workspace::create_alien_database_for_test(&alien)
        .await
        .expect("the alien database creates");
    std::fs::write(
        pointer_path(state_dir.path()),
        alien.to_string_lossy().as_bytes(),
    )
    .expect("the pointer writes");

    let (state, reopened) = booted_state(state_dir.path()).await;
    assert!(!reopened, "a refused file does not reopen");

    let current = current_file(state).await;
    assert_eq!(current["path"], serde_json::Value::Null);
    assert_eq!(current["grants"], serde_json::json!([]));
}

/// Reports whether the request included an `Authorization` header, so
/// the client tests can observe what was sent.
async fn mock_auth_probe(headers: HeaderMap) -> Response {
    let body = if headers.contains_key(header::AUTHORIZATION) {
        "auth"
    } else {
        "no-auth"
    };
    ([(header::CONTENT_TYPE, "text/plain")], body).into_response()
}

#[tokio::test]
async fn empty_api_key_sends_no_authorization_header() {
    let base_url = spawn_gateway(Router::new().route("/v1/models", get(mock_auth_probe))).await;
    let anonymous = GatewayClient::new(&base_url, "").expect("client builds");
    let response = anonymous.list_models().await.expect("request completes");
    assert_eq!(response.body, b"no-auth", "empty key sends no header");

    let keyed = GatewayClient::new(&base_url, "test-key").expect("client builds");
    let response = keyed.list_models().await.expect("request completes");
    assert_eq!(response.body, b"auth", "a set key still authenticates");
}

#[test]
fn default_bind_is_loopback_port_7910() {
    assert_eq!(DEFAULT_ADDR, "127.0.0.1:7910");
}

#[test]
fn the_relay_deadline_outlasts_the_gateway_request_timeout() {
    assert!(
        workshop_support::RELAY_DEADLINE > workshop_gateway::REQUEST_TIMEOUT,
        "the route deadline must let the gateway client time out first, \
         so the caller sees the relay's 502 rather than a blunt 408"
    );
}

#[test]
fn startup_sweeps_orphaned_temp_files_from_the_state_directory() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // Residue of a write that crashed between its temp file and its
    // rename in a previous run.
    let orphan = dir.path().join("workshop-state.json.42-7.pf-tmp");
    std::fs::write(&orphan, "partial").expect("the simulated crash residue writes");
    let config = config_for("http://127.0.0.1:1", dir.path());
    let gateway = ResolvedGateway::from_config(&config.gateway);
    let _state = state_with_gateway(&config, &gateway).expect("state builds");
    assert!(
        !orphan.exists(),
        "state construction sweeps orphaned temp files from the state directory"
    );
}

/// A plain GET to `/ws` without upgrade headers is rejected with 400,
/// which proves the sessions subsystem's route is mounted through the
/// registry; the WebSocket flow is covered by the integration binary's
/// `session` modules over a live socket.
#[tokio::test]
async fn ws_route_rejects_a_non_upgrade_get() {
    use tower::ServiceExt as _;

    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = axum::http::Request::builder()
        .uri("/ws")
        .body(axum::body::Body::empty())
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
}

/// The excised buffered chat endpoint is gone from the router: a
/// `POST /chat` answers 404.
#[tokio::test]
async fn post_chat_is_absent_and_answers_not_found() {
    use tower::ServiceExt as _;

    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/chat")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}]}"#,
        ))
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
}
