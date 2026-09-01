//! Status and snapshot behavior of the `/ws` workshop socket: status
//! frames riding the socket, the retained status, catalog, and workbench
//! snapshots on connect, the malformed- and unknown-frame refusals, and
//! the catalog push on reconnect.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::routing::get;
use futures_util::SinkExt;
use serde_json::json;
use tokio_tungstenite::tungstenite;

use promptforge_workshop_server::fixtures::{
    Activity, Progress, ReconnectBackoff, Severity, StatusBarUpdate, spawn_heartbeat,
};

use crate::common::{JsonSocket, TestServer, spawn_gateway};

use super::{
    flippable_health, mock_models, read_frame, read_non_status_frame, spawn_session_server,
};

#[tokio::test]
async fn a_new_connection_receives_the_current_status_as_its_first_frame() {
    // Nothing listens on port 1: after the heartbeat's first probe the
    // status settles on "Gateway unreachable" and never changes again, so
    // the frame below is deterministic.
    let server = TestServer::spawn("http://127.0.0.1:1");
    let mut witness = JsonSocket::connect(&server.ws_url("/ws")).await;
    witness
        .recv_until(Duration::from_secs(10), |frame| {
            frame["type"] == "status" && frame["label"] == "Gateway unreachable"
        })
        .await;

    // The outage predates this connection, so only the retained snapshot
    // can deliver it here - the contract's resend of the current status
    // on reconnect.
    let mut late = JsonSocket::connect(&server.ws_url("/ws")).await;
    let snapshot = late.recv_json().await;
    assert_eq!(
        snapshot,
        json!({
            "type": "status",
            "label": "Gateway unreachable",
            "description": "the gateway does not answer its health probe",
            "progress": null,
            "severity": "info",
            "activity": "general",
        }),
        "the retained status arrives as the connection's first frame"
    );
    late.close().await;
    witness.close().await;
}

#[tokio::test]
async fn status_updates_reach_connected_sessions_as_status_frames() {
    let (url, _state_dir, state) = spawn_session_server("http://127.0.0.1:1").await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    // A malformed frame's error reply proves the session loop is
    // running (the connect snapshot rides ahead of it and is skipped).
    socket
        .send(tungstenite::Message::Text("not json".into()))
        .await
        .expect("the frame is sent");
    let reply = read_non_status_frame(&mut socket).await;
    assert_eq!(reply["type"], "error");

    state.status().emit(StatusBarUpdate {
        label: "Downloading model".to_string(),
        description: "ggml-large-v3.bin".to_string(),
        progress: Some(Progress {
            current: 1,
            total: 2,
        }),
        severity: Severity::Info,
        activity: Activity::Generating,
    });

    let frame = read_frame(&mut socket).await;
    assert_eq!(
        frame,
        serde_json::json!({
            "type": "status",
            "label": "Downloading model",
            "description": "ggml-large-v3.bin",
            "progress": {"current": 1, "total": 2},
            "severity": "info",
            "activity": "generating",
        }),
        "the update arrives as one status frame"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn an_unknown_frame_type_is_refused_with_an_error_frame() {
    let (url, _state_dir, _state) = spawn_session_server("http://127.0.0.1:1").await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    // The excised chat frame is now just an unknown type: the session
    // answers with a refusal naming the two menu events and survives.
    let stale = serde_json::json!({
        "type": "chat",
        "id": 7,
        "model": "test-model",
        "messages": [{"role": "user", "content": "ping"}],
    })
    .to_string();
    socket
        .send(tungstenite::Message::Text(stale.into()))
        .await
        .expect("the frame is sent");
    let reply = read_non_status_frame(&mut socket).await;
    assert_eq!(reply["type"], "error");
    assert_eq!(reply["id"], 7, "the refusal echoes the frame id");
    let message = reply["message"]
        .as_str()
        .expect("the refusal names the accepted frame types");
    assert!(
        message.contains("select_model") && message.contains("switch_profile"),
        "the refusal names the two menu events: {message}"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_new_session_receives_the_retained_status_and_catalog_snapshots() {
    let (url, _state_dir, state) = spawn_session_server("http://127.0.0.1:1").await;
    // Both pushes land on the buses while nobody is connected, so only
    // the retained copies can deliver them to the socket below - the
    // contract's resend-on-reconnect for ephemeral frames.
    state.status().emit(StatusBarUpdate {
        label: "Downloading model".to_string(),
        description: "ggml-large-v3.bin".to_string(),
        progress: Some(Progress {
            current: 1,
            total: 2,
        }),
        severity: Severity::Info,
        activity: Activity::Generating,
    });
    state.catalog().publish(vec![
        serde_json::json!({"id": "test-model", "object": "model"}),
    ]);

    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    let first = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
        .await
        .expect("the status snapshot arrives unprompted");
    assert_eq!(
        first,
        serde_json::json!({
            "type": "status",
            "label": "Downloading model",
            "description": "ggml-large-v3.bin",
            "progress": {"current": 1, "total": 2},
            "severity": "info",
            "activity": "generating",
        }),
        "the retained status is the connection's first frame"
    );
    let second = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
        .await
        .expect("the catalog snapshot arrives unprompted");
    assert_eq!(
        second,
        serde_json::json!({
            "type": "models",
            "models": [{"id": "test-model", "object": "model"}],
        }),
        "the retained catalog follows it"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_new_session_receives_the_retained_workbench_snapshot() {
    let (url, _state_dir, state) = spawn_session_server("http://127.0.0.1:1").await;
    // Both retained copies exist before anyone connects, so only the
    // connect-time sends can deliver them below - the boot-with-zero-
    // HTTP-fetches promise.
    state
        .catalog()
        .publish(vec![serde_json::json!({"id": "test-model"})]);
    state.menu().set_profiles(
        vec!["main".to_string(), "coding".to_string()],
        Some("main".to_string()),
    );

    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    let first = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
        .await
        .expect("the status snapshot arrives unprompted");
    assert_eq!(first["type"], "status", "the retained status arrives first");
    let second = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
        .await
        .expect("the catalog snapshot arrives unprompted");
    assert_eq!(second["type"], "models", "the retained catalog follows");
    let third = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
        .await
        .expect("the workbench snapshot arrives unprompted");
    assert_eq!(
        third,
        serde_json::json!({
            "type": "workbench",
            "profiles": ["main", "coding"],
            "active": "main",
            "switching": null,
            "selected": null,
            "chat_ready": false,
        }),
        "the retained workbench snapshot completes the boot state"
    );
    socket.close(None).await.expect("close the socket");
}

// Un-ignored with the session rewrite: the flip below now waits for
// the heartbeat's observed outage, so the recovery is always a real
// down-to-up transition and the catalog push always happens; the
// session's own subscription is live before the flip for the same
// reason.
#[tokio::test]
async fn a_gateway_reconnect_pushes_the_refreshed_catalog_to_sessions() {
    let healthy = Arc::new(AtomicBool::new(false));
    let base_url = spawn_gateway(
        Router::new()
            .route("/health", get(flippable_health))
            .route("/v1/models", get(mock_models))
            .with_state(Arc::clone(&healthy)),
    )
    .await;
    let (url, _state_dir, state) = spawn_session_server(&base_url).await;
    let heartbeat = spawn_heartbeat(
        state.gateway_client().clone(),
        state.push(),
        state.health().clone(),
        Duration::from_millis(25),
        // A fast schedule so the down-phase retry lands within a few
        // ticks instead of the production seconds.
        ReconnectBackoff::with_schedule(
            Duration::from_millis(10),
            Duration::from_millis(40),
            Duration::from_secs(60),
        ),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    // Hold the flip until the heartbeat's outage reaches this socket
    // (as the connect snapshot or a live push): the catalog is pushed
    // only on an observed down-to-up transition, so flipping before
    // the first probe lands would leave nothing to push.
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), read_frame(&mut socket))
            .await
            .expect("the heartbeat publishes the outage within the deadline");
        if frame["type"] == "status" && frame["label"] == "Gateway unreachable" {
            break;
        }
    }

    healthy.store(true, Ordering::Relaxed);
    // Status frames (the "Connected to gateway" transition) interleave
    // with the push; read until the models frame arrives.
    let frame = loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), read_frame(&mut socket))
            .await
            .expect("frames keep arriving within the deadline");
        if frame["type"] == "models" {
            break frame;
        }
    };
    assert_eq!(
        frame,
        serde_json::json!({
            "type": "models",
            "models": [{"id": "test-model", "object": "model", "owned_by": "promptforge"}],
        }),
        "the refreshed catalog arrives as one models frame"
    );
    socket.close(None).await.expect("close the socket");
    heartbeat.shutdown().await;
}
