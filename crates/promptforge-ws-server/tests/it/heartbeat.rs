//! Characterization tests for the heartbeat-driven frames on the chat
//! socket: the fail-fast error frame while the gateway is known down, and
//! the refreshed catalog push on reconnect.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;

use crate::common::{JsonSocket, TestServer, spawn_gateway};

/// The catalog the mock gateway serves from `/v1/models`.
const CATALOG: &str =
    r#"{"object":"list","data":[{"id":"test-model","object":"model","owned_by":"promptforge"}]}"#;

/// A mock `/health` whose answer flips under test control.
async fn flippable_health(State(healthy): State<Arc<AtomicBool>>) -> Response {
    if healthy.load(Ordering::Relaxed) {
        StatusCode::OK.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

/// A static mock catalog for the reconnect push.
async fn mock_models() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], CATALOG).into_response()
}

/// Sends chat frames tagged with `id` until one is answered with the
/// fail-fast "Gateway unreachable" error, proving the heartbeat has
/// published the outage; earlier frames can race the first probe and be
/// answered with ordinary upstream errors instead.
#[expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]
async fn await_gateway_known_down(socket: &mut JsonSocket, id: u64) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            socket
                .send_json(&json!({
                    "type": "chat",
                    "id": id,
                    "model": "test-model",
                    "messages": [{"role": "user", "content": "ping"}],
                }))
                .await;
            let reply = socket.recv_non_status().await;
            assert_eq!(
                reply["type"], "error",
                "a chat against a down gateway is answered with an error frame: {reply}"
            );
            if reply["message"] == "Gateway unreachable" {
                break reply;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the heartbeat publishes the outage within the deadline")
}

#[tokio::test]
async fn a_gateway_known_down_short_circuits_chat_with_an_error_frame() {
    // Nothing listens on port 1, so the probe and any raced chat attempts
    // fail deterministically.
    let server = TestServer::spawn("http://127.0.0.1:1");
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;

    let reply = await_gateway_known_down(&mut socket, 7).await;
    assert_eq!(
        reply,
        json!({"type": "error", "message": "Gateway unreachable", "id": 7}),
        "the chat fails fast, with the request id echoed"
    );
    // A transport failure before the stream opens tapes nothing either, so
    // the raced attempts leave no events behind.
    assert!(
        server.tape_events().is_empty(),
        "no upstream attempt means no tape event"
    );
    socket.close().await;
}

#[tokio::test]
async fn a_gateway_reconnect_pushes_the_refreshed_catalog() {
    let healthy = Arc::new(AtomicBool::new(false));
    let gateway = Router::new()
        .route("/health", get(flippable_health))
        .route("/v1/models", get(mock_models))
        .with_state(Arc::clone(&healthy));
    let base_url = spawn_gateway(gateway).await;
    let server = TestServer::spawn(&base_url);
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;
    // Hold the flip until the outage is observable, so the recovery is a
    // real down-to-up transition; the initial connect pushes no catalog.
    await_gateway_known_down(&mut socket, 9).await;

    healthy.store(true, Ordering::Relaxed);
    // The next probe lands within the 5 s heartbeat interval and the
    // catalog rides behind the "Connected to gateway" status frame.
    let frame = socket
        .recv_until(Duration::from_secs(30), |frame| frame["type"] == "models")
        .await;
    assert_eq!(
        frame,
        json!({
            "type": "models",
            "models": [{"id": "test-model", "object": "model", "owned_by": "promptforge"}],
        }),
        "the refreshed catalog arrives as one models frame"
    );
    socket.close().await;
}
