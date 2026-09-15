use super::*;

use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::fixtures::{config_for, spawn_gateway, state_for};
use crate::gateway::GatewayClient;

/// Reports whether the request carried an `Authorization` header, so
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
        workshop_support::RELAY_DEADLINE > crate::gateway::REQUEST_TIMEOUT,
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
/// `POST /chat` answers 404, not a relay response.
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
