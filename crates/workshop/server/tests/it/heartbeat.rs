//! Characterization tests for the heartbeat-driven frames on the workshop
//! socket: the outage status while the gateway is known down, and the
//! refreshed catalog push on reconnect.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;

use crate::common::{JsonSocket, RECV_TIMEOUT, TestServer, spawn_gateway};

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

/// Waits until the heartbeat's observed outage reaches this socket as
/// the "Gateway unreachable" status frame - the connect snapshot or a
/// live push, whichever lands first.
async fn await_gateway_known_down(socket: &mut JsonSocket) {
    socket
        .recv_until(Duration::from_secs(10), |frame| {
            frame["type"] == "status" && frame["label"] == "Gateway unreachable"
        })
        .await;
}

#[tokio::test]
async fn a_gateway_known_down_publishes_the_outage_status() {
    // Nothing listens on port 1, so the probe fails deterministically.
    let server = TestServer::spawn("http://127.0.0.1:1");
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;
    await_gateway_known_down(&mut socket).await;
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
    await_gateway_known_down(&mut socket).await;

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

    // The push above happened before this connection existed, so only the
    // retained snapshot can deliver the catalog here - the contract's
    // resend of the catalog on reconnect.
    let mut late = JsonSocket::connect(&server.ws_url("/ws")).await;
    let resent = late
        .recv_until(RECV_TIMEOUT, |frame| frame["type"] == "models")
        .await;
    assert_eq!(
        resent,
        json!({
            "type": "models",
            "models": [{"id": "test-model", "object": "model", "owned_by": "promptforge"}],
        }),
        "a connection made after the push still receives the catalog"
    );
    late.close().await;
    socket.close().await;
}
