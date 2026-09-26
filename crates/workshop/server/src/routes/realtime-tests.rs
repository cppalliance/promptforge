//! Realtime relay refusal tests: the origin allowlist and the
//! no-subprotocol rule, asserted against a live server so each refusal's
//! status and body are pinned exactly as the browser sees them.

use axum::http::StatusCode;
use tokio_tungstenite::tungstenite::Error as SocketError;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

use crate::app::test_helpers::config_for;

/// Builds a `/v1/realtime` upgrade request with an optional `Origin` and an
/// optional requested subprotocol.
fn request_with(
    base: &str,
    origin: Option<&str>,
    subprotocol: Option<&str>,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let address = base
        .strip_prefix("http://")
        .expect("the server URL is http");
    let mut request = format!("ws://{address}/v1/realtime")
        .into_client_request()
        .expect("the WebSocket request builds");
    if let Some(origin) = origin {
        request.headers_mut().insert(
            "origin",
            origin
                .parse()
                .expect("the test Origin is a valid header value"),
        );
    }
    if let Some(subprotocol) = subprotocol {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            subprotocol
                .parse()
                .expect("the test subprotocol is a valid header value"),
        );
    }
    request
}

/// Runs the handshake and returns the HTTP response that refused it.
async fn refused_response(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
) -> tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>> {
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("the WebSocket handshake is refused");
    let SocketError::Http(response) = error else {
        panic!("the refusal is an HTTP response, got {error:?}");
    };
    *response
}

#[tokio::test]
async fn a_foreign_origin_is_refused_with_an_empty_forbidden_body() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let mut config = config_for("http://127.0.0.1:1", state_dir.path());
    config.server.bind = "127.0.0.1:0".to_string();
    let server = crate::serve::spawn_resolved(config).expect("server spawns");

    let response = refused_response(request_with(
        server.url(),
        Some("https://evil.example"),
        None,
    ))
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        response.body().as_deref().is_none_or(<[u8]>::is_empty),
        "the refusal body is empty"
    );

    server.shutdown().expect("graceful shutdown succeeds");
}

#[tokio::test]
async fn a_requested_subprotocol_is_refused_with_an_empty_bad_request_body() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let mut config = config_for("http://127.0.0.1:1", state_dir.path());
    config.server.bind = "127.0.0.1:0".to_string();
    let server = crate::serve::spawn_resolved(config).expect("server spawns");

    let response = refused_response(request_with(server.url(), None, Some("realtime"))).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        response.body().as_deref().is_none_or(<[u8]>::is_empty),
        "the refusal body is empty"
    );

    server.shutdown().expect("graceful shutdown succeeds");
}
