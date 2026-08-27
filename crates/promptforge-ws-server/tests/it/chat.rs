//! Characterization tests for the `/ws` chat socket: the delta, reasoning,
//! done, and error frame sequences, the unsolicited status frames riding
//! the same socket, and disconnect cleanup, pinned end to end.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use futures_util::stream;
use serde_json::json;

use crate::common::{JsonSocket, TestServer, spawn_gateway};

/// The SSE stream of a plain answer: a role-priming event with no content,
/// two content deltas, and the terminal sentinel.
const STREAM_BODY: &str = concat!(
    "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
    "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"po\"}}]}\n\n",
    "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ng\"}}]}\n\n",
    "data: [DONE]\n\n",
);

/// A reasoning model's stream: scratch work on the side channel first,
/// then the answer content.
const REASONING_STREAM_BODY: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hmm \"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"okay\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"po\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ng\"}}]}\n\n",
    "data: [DONE]\n\n",
);

/// Answers the heartbeat's health probe so the gateway stays reachable.
async fn health_ok() -> StatusCode {
    StatusCode::OK
}

/// A mock gateway streaming `body` for every chat completion, healthy to
/// the heartbeat.
fn streaming_gateway(body: &'static str) -> Router {
    Router::new().route("/health", get(health_ok)).route(
        "/v1/chat/completions",
        post(move || async move {
            ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
        }),
    )
}

/// A mock gateway dripping one delta every 50 ms, giving a client time to
/// disconnect mid-stream before the drip runs out.
fn dripping_gateway() -> Router {
    Router::new().route("/health", get(health_ok)).route(
        "/v1/chat/completions",
        post(|| async {
            let chunks = stream::unfold(0u8, |step| async move {
                if step >= 40 {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                let payload = format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"x{step}\"}}}}]}}\n\n"
                );
                Some((
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(payload)),
                    step + 1,
                ))
            });
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(chunks),
            )
                .into_response()
        }),
    )
}

/// Sends one well-formed chat frame naming the test model, tagged with
/// `id`.
async fn send_chat(socket: &mut JsonSocket, id: u64) {
    socket
        .send_json(&json!({
            "type": "chat",
            "id": id,
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        }))
        .await;
}

#[tokio::test]
async fn a_chat_streams_deltas_in_order_then_done_and_tapes_the_exchange() {
    let base_url = spawn_gateway(streaming_gateway(STREAM_BODY)).await;
    let server = TestServer::spawn(&base_url);
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;
    send_chat(&mut socket, 1).await;

    // The role-priming event carries no content and yields no frame; every
    // reply frame echoes the request id.
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "po", "id": 1})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "ng", "id": 1})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "done", "id": 1})
    );

    // The terminal frame follows the tape write, so holding `done` means
    // the tape is durable.
    let events = server.tape_events();
    assert_eq!(events.len(), 1, "exactly one tape event per chat frame");
    assert_eq!(events[0]["model"], "test-model");
    assert_eq!(
        events[0]["response"], "pong",
        "the tape holds the assembled content, not the raw frames"
    );
    socket.close().await;
}

#[tokio::test]
async fn reasoning_deltas_arrive_as_reasoning_frames_and_stay_off_the_tape() {
    let base_url = spawn_gateway(streaming_gateway(REASONING_STREAM_BODY)).await;
    let server = TestServer::spawn(&base_url);
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;
    // Untagged on purpose: an absent id is omitted from every reply frame,
    // not serialized as null.
    socket
        .send_json(&json!({
            "type": "chat",
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        }))
        .await;

    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "reasoning", "content": "hmm "}),
        "the reasoning side channel arrives as reasoning frames"
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "reasoning", "content": "okay"})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "po"})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "ng"})
    );
    assert_eq!(socket.recv_non_status().await, json!({"type": "done"}));

    let events = server.tape_events();
    assert_eq!(events.len(), 1, "exactly one tape event per chat frame");
    assert_eq!(
        events[0]["response"], "pong",
        "the tape holds the answer content only, never the reasoning"
    );
    socket.close().await;
}

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
    send_chat(&mut socket, 1).await;

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
async fn a_malformed_frame_is_answered_with_an_error_and_the_session_survives() {
    let base_url = spawn_gateway(streaming_gateway(STREAM_BODY)).await;
    let server = TestServer::spawn(&base_url);
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;

    for bad in [
        "not json",
        r#"{"type":"bogus"}"#,
        r#"{"type":"chat","model":"test-model"}"#,
    ] {
        socket.send_text(bad).await;
        let frame = socket.recv_non_status().await;
        assert_eq!(
            frame["type"], "error",
            "a malformed frame is answered, not fatal: {bad}"
        );
        assert!(
            frame["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "the error frame names the failure: {frame}"
        );
    }

    // The session survives: a well-formed frame still streams a full reply.
    send_chat(&mut socket, 2).await;
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "po", "id": 2})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "ng", "id": 2})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "done", "id": 2})
    );
    socket.close().await;
}

#[tokio::test]
async fn a_client_disconnect_mid_stream_returns_status_to_ready_and_tapes_the_note() {
    let base_url = spawn_gateway(dripping_gateway()).await;
    let server = TestServer::spawn(&base_url);
    // A second session observes the status bus: the dead client's terminal
    // update must still reach every remaining subscriber.
    let mut observer = JsonSocket::connect(&server.ws_url("/ws")).await;
    // Consume the observer's connect snapshot, which may already read
    // Ready, so the Ready watched for below can only be the
    // post-disconnect idle. (Licensed by the protocol contract: the
    // current status is resent on reconnect.)
    let snapshot = observer.recv_json().await;
    assert_eq!(snapshot["type"], "status", "the snapshot arrives first");
    let mut chatter = JsonSocket::connect(&server.ws_url("/ws")).await;
    send_chat(&mut chatter, 3).await;
    let first = chatter.recv_non_status().await;
    assert_eq!(first["type"], "delta");
    // Drop the socket without a close handshake; the server notices when a
    // later delta send fails.
    drop(chatter);

    // The observer sees the relay return to Ready once the failed send
    // ends the stream, rather than keeping a stale activity LED.
    let idle = observer
        .recv_until(Duration::from_secs(10), |frame| {
            frame["type"] == "status" && frame["label"] == "Ready"
        })
        .await;
    assert_eq!(idle["activity"], "general");
    assert_eq!(idle["severity"], "info");

    // The idle push follows the tape write, so the note is durable here.
    let events = server.tape_events();
    assert_eq!(events.len(), 1, "a mid-stream disconnect tapes one event");
    assert_eq!(
        events[0]["response"]["error"], "client disconnected mid-stream",
        "the disconnect is taped as an error note"
    );
    let partial = events[0]["response"]["content"]
        .as_str()
        .expect("the partial content is a string");
    assert!(
        partial.starts_with("x0"),
        "the partial content is taped alongside: {partial:?}"
    );
    observer.close().await;
}
