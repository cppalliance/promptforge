//! Cancellation and admission behavior of the `/ws` chat socket: a cancel
//! tearing down one chat while the rest stream on, a cancel of a chat
//! parked at gateway admission, and a parked open never blocking the
//! socket.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures_util::{SinkExt, stream};
use tokio_tungstenite::tungstenite;

use crate::common::spawn_gateway;

use super::{
    STREAM_BODY, authorized, mock_chat_stream, read_non_status_frame, replies_until_dones,
    send_cancel, send_chat, send_tagged_chat, spawn_chat_server, spawn_indexed_drip_server,
    tape_events, tape_events_when,
};

/// A mock gateway whose admission is scripted by the request's user
/// message: `"drip"` streams a long drip immediately, `"park"` holds
/// the response headers - no bytes at all, the exact shape of a
/// request waiting in a per-dominion queue at capacity - until the
/// test fires the Notify, then streams `STREAM_BODY`.
async fn mock_chat_stream_admission(
    State(gate): State<Arc<tokio::sync::Notify>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    assert!(authorized(&headers));
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    assert_eq!(body["stream"], true, "the stream flag is forwarded");
    if body["messages"][0]["content"] == "drip" {
        let chunks = stream::unfold(0u8, |step| async move {
            if step >= 40 {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let payload =
                format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"x{step}\"}}}}]}}\n\n");
            Some((
                Ok::<_, std::io::Error>(axum::body::Bytes::from(payload)),
                step + 1,
            ))
        });
        return (
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(chunks),
        )
            .into_response();
    }
    if body["messages"][0]["content"] == "park" {
        gate.notified().await;
    }
    ([(header::CONTENT_TYPE, "text/event-stream")], STREAM_BODY).into_response()
}

/// Spawns the chat server against the scripted-admission gateway,
/// returning the release handle for its parked requests.
async fn spawn_admission_server() -> (String, tempfile::TempDir, Arc<tokio::sync::Notify>) {
    let gate = Arc::new(tokio::sync::Notify::new());
    let base_url = spawn_gateway(
        Router::new()
            .route("/v1/chat/completions", post(mock_chat_stream_admission))
            .with_state(Arc::clone(&gate)),
    )
    .await;
    let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
    (url, tape_dir, gate)
}

/// Sends one chat frame tagged with `id` whose user message is
/// `content`, scripting the admission mock's behavior.
async fn send_marked_chat<S>(socket: &mut S, id: u64, content: &str)
where
    S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
{
    let frame = serde_json::json!({
        "type": "chat",
        "id": id,
        "model": "test-model",
        "messages": [{"role": "user", "content": content}],
    })
    .to_string();
    socket
        .send(tungstenite::Message::Text(frame.into()))
        .await
        .expect("the chat frame is sent");
}

#[tokio::test]
async fn a_cancel_ends_one_chat_while_the_other_streams_to_completion() {
    let (url, tape_dir, _state) = spawn_indexed_drip_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    // Chat 1 gets the long stream; chat 2's short stream outlives the
    // cancel below, so it settles after chat 1 is torn down.
    send_tagged_chat(&mut socket, 1).await;
    send_tagged_chat(&mut socket, 2).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = read_non_status_frame(&mut socket).await;
            if frame["type"] == "delta" && frame["id"] == 1 {
                break;
            }
        }
    })
    .await
    .expect("chat 1 streams before the cancel");
    send_cancel(&mut socket, 1).await;

    // Chat 2 streams to completion; chat 1 never settles on the wire.
    let replies = replies_until_dones(&mut socket, 1).await;
    let terminal = replies.last().expect("the done frame was collected");
    assert_eq!(
        terminal["id"], 2,
        "the surviving chat's terminal is the only done: {replies:?}"
    );

    // The canceled chat's tape write precedes the cancel frame's
    // handling returning, and chat 2's precedes its done, so both are
    // durable here.
    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 2, "both chats tape exactly one event each");
    let canceled = events
        .iter()
        .find(|event| event["response"]["error"] == "chat canceled by client")
        .expect("the canceled chat taped the abandonment");
    assert!(
        canceled["response"]["content"]
            .as_str()
            .expect("the partial content is a string")
            .starts_with("c0-"),
        "the partial content is taped beside the note: {canceled}"
    );
    assert!(
        events
            .iter()
            .any(|event| event["response"] == "c1-0c1-1c1-2c1-3"),
        "the surviving chat taped its full assembly: {events:?}"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_chat_parked_at_gateway_admission_never_blocks_the_session() {
    let (url, tape_dir, gate) = spawn_admission_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_marked_chat(&mut socket, 1, "drip").await;
    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(first["id"], 1, "chat 1 streams before chat 2 is sent");

    // Chat 2 posts and parks: the mock holds its response headers,
    // exactly as the gateway's queue does at max_concurrency.
    send_marked_chat(&mut socket, 2, "park").await;
    // Chat 1's deltas keep flowing while chat 2 waits for admission:
    // the session loop did not block on the parked open.
    for _ in 0..3 {
        let frame =
            tokio::time::timeout(Duration::from_secs(10), read_non_status_frame(&mut socket))
                .await
                .expect("chat 1 keeps streaming while chat 2 is parked");
        assert_eq!(frame["type"], "delta");
        assert_eq!(frame["id"], 1, "only chat 1 streams while chat 2 is parked");
    }

    // A cancel for chat 1 - the one action that frees real capacity -
    // is read and processed while chat 2 is still parked: its tape
    // note lands without any release of the gate.
    send_cancel(&mut socket, 1).await;
    let events = tape_events_when(&tape_dir, 1).await;
    assert_eq!(
        events[0]["response"]["error"], "chat canceled by client",
        "the cancel settles while chat 2 waits for admission: {events:?}"
    );

    // Release chat 2's admission; it streams to completion.
    gate.notify_one();
    let replies = replies_until_dones(&mut socket, 1).await;
    let terminal = replies.last().expect("the done frame was collected");
    assert_eq!(
        terminal["id"], 2,
        "chat 2 settles once admitted: {replies:?}"
    );
    assert!(
        replies
            .iter()
            .any(|frame| frame["type"] == "delta" && frame["id"] == 2),
        "chat 2 streamed its deltas after release: {replies:?}"
    );
    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 2, "one tape event per chat");
    assert!(
        events.iter().any(|event| event["response"] == "pong"),
        "the released chat taped its full assembly: {events:?}"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_cancel_for_a_chat_still_opening_removes_it_cleanly() {
    let (url, tape_dir, _gate) = spawn_admission_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    // Chat 1 parks before its headers and is canceled there; the
    // gate is never released, so only the cancel can settle it.
    send_marked_chat(&mut socket, 1, "park").await;
    send_cancel(&mut socket, 1).await;

    // The cancel tapes the abandonment exactly once, with nothing
    // streamed yet.
    let events = tape_events_when(&tape_dir, 1).await;
    assert_eq!(events.len(), 1, "the canceled open tapes exactly once");
    assert_eq!(
        events[0]["response"]["error"], "chat canceled by client",
        "the abandonment is taped: {events:?}"
    );
    assert_eq!(
        events[0]["response"]["content"], "",
        "a chat canceled while opening streamed nothing"
    );

    // The canceled chat produces no frames afterward: a fresh chat
    // is admitted immediately (only "park" requests are held) and
    // every reply frame carries its id alone.
    send_marked_chat(&mut socket, 2, "ping").await;
    let replies = replies_until_dones(&mut socket, 1).await;
    assert!(
        replies.iter().all(|frame| frame["id"] == 2),
        "no frame of the canceled chat ever arrives: {replies:?}"
    );
    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 2, "the canceled chat's note stays single");
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_cancel_for_an_unknown_id_is_ignored() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream))).await;
    let (url, _tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_cancel(&mut socket, 99).await;
    // Replies are ordered, so if the cancel had drawn an error frame
    // it would arrive ahead of this chat's first delta.
    send_chat(&mut socket).await;
    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(
        first,
        serde_json::json!({"type": "delta", "content": "po"}),
        "the unknown cancel drew no reply and the session streams on"
    );
    replies_until_dones(&mut socket, 1).await;
    socket.close(None).await.expect("close the socket");
}
