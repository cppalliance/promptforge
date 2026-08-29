//! Status and snapshot behavior of the `/ws` chat socket: status frames
//! riding the socket, the retained status, catalog, and workbench
//! snapshots on connect, the fail-fast frame while the gateway is known
//! down, and the catalog push on reconnect.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};
use futures_util::SinkExt;
use serde_json::json;
use tokio_tungstenite::tungstenite;

use promptforge_workshop_server::fixtures::{
    Activity, Progress, ReconnectBackoff, Severity, StatusBarUpdate, spawn_heartbeat,
};

use crate::common::{JsonSocket, TestServer, spawn_gateway};

use super::{
    STREAM_BODY, flippable_health, mock_chat_stream, mock_models, read_frame,
    read_non_status_frame, send_chat, send_chat_json, spawn_chat_server, streaming_gateway,
};

#[tokio::test]
async fn a_chat_pushes_status_frames_on_the_same_socket() {
    let base_url = spawn_gateway(streaming_gateway(STREAM_BODY)).await;
    let server = TestServer::spawn(&base_url);
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;
    // Every session's first frame is the retained status snapshot, which
    // may already read Ready; consume it so the Ready watched for below
    // can only be the post-chat idle. (Licensed by the protocol contract:
    // the current status is resent on reconnect.)
    let snapshot = socket.recv_json().await;
    assert_eq!(snapshot["type"], "status", "the snapshot arrives first");
    send_chat_json(&mut socket, 1).await;

    // Collect the chat's status frames up to the terminal idle push; the
    // idle frame follows `done` on the bus, so reading until Ready sees
    // the whole sequence.
    let mut labels: Vec<String> = Vec::new();
    let mut saw_generating_pulse = false;
    let idle = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = socket.recv_json().await;
            if frame["type"] != "status" {
                continue;
            }
            if frame["severity"] == "debug" && frame["activity"] == "generating" {
                saw_generating_pulse = true;
            }
            let label = frame["label"]
                .as_str()
                .expect("a status frame carries a label")
                .to_string();
            if label == "Ready" {
                break frame;
            }
            labels.push(label);
        }
    })
    .await
    .expect("the idle frame arrives after the chat settles");

    assert_eq!(
        idle,
        json!({
            "type": "status",
            "label": "Ready",
            "description": "idle",
            "progress": null,
            "severity": "info",
            "activity": "general",
        }),
        "the resting status frame arrives with the full wire shape"
    );
    assert!(
        labels.iter().any(|label| label.contains("Submitting")),
        "a Submitting status frame arrived: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.contains("Streaming")),
        "a Streaming status frame arrived: {labels:?}"
    );
    assert!(
        saw_generating_pulse,
        "a debug-severity pulse with the generating activity drove the LED"
    );
    socket.close().await;
}

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
    let (url, _tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
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
async fn a_new_session_receives_the_retained_status_and_catalog_snapshots() {
    let (url, _tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
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
    let (url, _tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
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

#[tokio::test]
async fn a_gateway_known_down_short_circuits_chat_with_an_error_frame() {
    let (url, tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
    state.health().publish(false);
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    let frame = serde_json::json!({
        "type": "chat",
        "id": 7,
        "model": "test-model",
        "messages": [{"role": "user", "content": "ping"}],
    })
    .to_string();
    socket
        .send(tungstenite::Message::Text(frame.into()))
        .await
        .expect("the chat frame is sent");

    let reply = read_non_status_frame(&mut socket).await;
    assert_eq!(
        reply,
        serde_json::json!({"type": "error", "message": "Gateway unreachable", "id": 7}),
        "the chat fails fast, with the request id echoed"
    );
    socket.close(None).await.expect("close the socket");
    let raw = std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
    assert!(
        raw.trim().is_empty(),
        "no upstream attempt means no tape event"
    );
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
    let (url, _tape_dir, state) = spawn_chat_server(&base_url).await;
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

#[tokio::test]
async fn a_chat_reports_submitting_then_streaming() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream))).await;
    let (url, _tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;

    let mut labels: Vec<String> = Vec::new();
    loop {
        let frame = read_frame(&mut socket).await;
        match frame["type"].as_str() {
            Some("status") => labels.push(
                frame["label"]
                    .as_str()
                    .expect("a status frame carries a label")
                    .to_string(),
            ),
            Some("done") => break,
            _ => {}
        }
    }
    assert!(
        labels.iter().any(|label| label.contains("Submitting")),
        "a Submitting status frame arrived: {labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.contains("Streaming")),
        "a Streaming status frame arrived: {labels:?}"
    );
    socket.close(None).await.expect("close the socket");
}
