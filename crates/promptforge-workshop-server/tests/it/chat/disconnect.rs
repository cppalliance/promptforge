//! Disconnect behavior of the `/ws` chat socket: a client vanishing
//! mid-stream ends the stream, tapes the abandonment beside the partial
//! content, and returns the status bar to Ready.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::stream;

use crate::common::{JsonSocket, TestServer, spawn_gateway};

use super::{
    authorized, health_ok, read_frame, read_non_status_frame, send_chat, send_chat_json,
    send_tagged_chat, spawn_chat_server,
};

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

/// Drips one delta every 50ms, giving a client time to disconnect
/// mid-stream before the drip runs out.
async fn mock_chat_stream_drips(headers: HeaderMap, body: String) -> Response {
    assert!(authorized(&headers));
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    assert_eq!(body["stream"], true, "the stream flag is forwarded");
    let chunks = stream::unfold(0u8, |step| async move {
        if step >= 40 {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let payload =
            format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"x{step}\"}}}}]}}\n\n");
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
    send_chat_json(&mut chatter, 3).await;
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

#[tokio::test]
async fn a_client_disconnect_mid_stream_is_taped_with_the_partial_content() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream_drips)))
            .await;
    let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
    // A second session observes the status bus: the dead client's
    // terminal update must still reach every remaining subscriber.
    let (mut observer, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect an observer to /ws");
    // Every session first receives the retained status snapshot -
    // Ready here, since the state is idle. Consume it so the Ready
    // watched for below can only be the post-disconnect idle.
    let snapshot = read_frame(&mut observer).await;
    assert_eq!(snapshot["type"], "status", "the snapshot arrives first");
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;
    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(first["type"], "delta");
    // Drop the socket without a close handshake; the server notices when
    // a later delta send fails.
    drop(socket);

    // The observer sees the relay return to Ready once the failed send
    // ends the stream, rather than keeping a stale activity LED.
    let idle = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let frame = read_frame(&mut observer).await;
            if frame["type"] == "status" && frame["label"] == "Ready" {
                break frame;
            }
        }
    })
    .await
    .expect("the observer sees the idle status after the disconnect");
    assert_eq!(idle["activity"], "general");
    assert_eq!(idle["severity"], "info");

    // The tape write follows the failed send, so poll for it.
    let mut events: Vec<serde_json::Value> = Vec::new();
    for _ in 0..100 {
        if let Ok(raw) = std::fs::read_to_string(tape_dir.path().join("tape.jsonl"))
            && !raw.trim().is_empty()
        {
            events = raw
                .lines()
                .map(|line| serde_json::from_str(line).expect("the tape line is valid JSON"))
                .collect();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
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
}

#[tokio::test]
async fn a_mid_stream_disconnect_tapes_every_in_flight_chats_note() {
    // The long drip on both requests keeps both chats mid-stream well
    // past the moment the failed send surfaces the disconnect.
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream_drips)))
            .await;
    let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_tagged_chat(&mut socket, 1).await;
    send_tagged_chat(&mut socket, 2).await;
    // One delta from each chat proves both streams are live.
    tokio::time::timeout(Duration::from_secs(10), async {
        let (mut live_one, mut live_two) = (false, false);
        while !(live_one && live_two) {
            let frame = read_non_status_frame(&mut socket).await;
            assert_eq!(frame["type"], "delta");
            match frame["id"].as_u64() {
                Some(1) => live_one = true,
                Some(2) => live_two = true,
                other => panic!("a delta of an unknown chat: {other:?}"),
            }
        }
    })
    .await
    .expect("both chats stream before the disconnect");
    // Drop the socket without a close handshake; the server notices
    // when a later delta send fails.
    drop(socket);

    // The tape writes follow the failed send, so poll for both.
    let mut events: Vec<serde_json::Value> = Vec::new();
    for _ in 0..100 {
        if let Ok(raw) = std::fs::read_to_string(tape_dir.path().join("tape.jsonl")) {
            events = raw
                .lines()
                .map(|line| serde_json::from_str(line).expect("the tape line is valid JSON"))
                .collect();
            if events.len() == 2 {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(events.len(), 2, "every in-flight chat tapes its own note");
    for event in &events {
        assert_eq!(
            event["response"]["error"], "client disconnected mid-stream",
            "each chat's abandonment is taped: {event}"
        );
    }
}
