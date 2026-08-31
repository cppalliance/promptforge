//! Characterization tests for the `/ws` chat socket: the delta, reasoning,
//! done, and error frame sequences, the unsolicited status frames riding
//! the same socket, and disconnect cleanup, pinned end to end.
//!
//! The root holds the shared harness - mock gateways, the server fixture,
//! frame readers and senders - and each child module pins one behavior
//! area of the socket.

// clippy.toml's allow-expect-in-tests covers #[test] functions only, not
// the helpers they share; failing a test by panicking with the invariant
// named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

mod cancellation;
mod disconnect;
mod menu;
mod multiplexing;
mod status;
mod stream;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite;

use promptforge_workshop_server::{
    AppState, Config, GatewayConfig, ServerConfig, TapeConfig, router,
};

use crate::common::{JsonSocket, spawn_gateway};

const STREAM_BODY: &str = concat!(
    "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
    "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"po\"}}]}\n\n",
    "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ng\"}}]}\n\n",
    "data: [DONE]\n\n",
);
const UPSTREAM_ERROR: &str =
    r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;

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

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer test-key")
}

async fn mock_chat_stream(headers: HeaderMap, body: String) -> Response {
    assert!(authorized(&headers));
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    assert_eq!(body["stream"], true, "the stream flag is forwarded");
    ([(header::CONTENT_TYPE, "text/event-stream")], STREAM_BODY).into_response()
}

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

/// Drips deltas whose content names the request's arrival order -
/// "c0-0", "c0-1", ... for the first request - one every 25 ms. The
/// first request drips eight chunks and every later one four, so two
/// overlapping chats always settle later-first: the first chat sent
/// outlives the second.
async fn mock_chat_stream_drips_indexed(
    State(counter): State<Arc<AtomicU64>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    assert!(authorized(&headers));
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    assert_eq!(body["stream"], true, "the stream flag is forwarded");
    let request = counter.fetch_add(1, Ordering::Relaxed);
    let chunks = if request == 0 { 8u64 } else { 4u64 };
    let drip = futures_util::stream::unfold(0u64, move |step| async move {
        if step >= chunks {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        let payload = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"c{request}-{step}\"}}}}]}}\n\n"
        );
        Some((
            Ok::<_, std::io::Error>(axum::body::Bytes::from(payload)),
            step + 1,
        ))
    });
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(drip),
    )
        .into_response()
}

/// Spawns the chat server against a gateway dripping indexed deltas
/// (first request long, later ones short).
async fn spawn_indexed_drip_server() -> (String, tempfile::TempDir, AppState) {
    let base_url = spawn_gateway(
        Router::new()
            .route("/v1/chat/completions", post(mock_chat_stream_drips_indexed))
            .with_state(Arc::new(AtomicU64::new(0))),
    )
    .await;
    spawn_chat_server(&base_url).await
}

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

/// A static mock catalog for the reconnect push test.
async fn mock_models() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], CATALOG).into_response()
}

/// Binds the workshop router against the gateway at `base_url` on a
/// free loopback port and returns the `/ws` URL, the tempdir keeping
/// the tape alive, and a handle on the shared state (for poking the
/// status and catalog buses directly).
async fn spawn_chat_server(base_url: &str) -> (String, tempfile::TempDir, AppState) {
    let tape_dir = tempfile::TempDir::new().expect("tempdir");
    let config = Config {
        gateway: GatewayConfig {
            base_url: base_url.to_string(),
            api_key: "test-key".to_string(),
        },
        tape: TapeConfig {
            path: tape_dir.path().join("tape.jsonl"),
        },
        server: ServerConfig::default(),
    };
    let state = AppState::new(&config).expect("state builds in tests");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the chat test server");
    let addr = listener.local_addr().expect("chat test server address");
    let served = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, router(served))
            .await
            .expect("chat test server serves");
    });
    (format!("ws://{addr}/ws"), tape_dir, state)
}

/// Reads one text frame from the client socket and parses it as JSON.
async fn read_frame<S>(socket: &mut S) -> serde_json::Value
where
    S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    let message = socket
        .next()
        .await
        .expect("a frame follows")
        .expect("the frame is not a socket error");
    let text = message.into_text().expect("the frame is text");
    serde_json::from_str(&text).expect("the frame is JSON")
}

/// Reads frames until one arrives that is not a status update. Status
/// frames are unsolicited - the snapshot on connect, then bus pushes
/// that may interleave with a chat's replies at any point - so reply
/// assertions skip them.
async fn read_non_status_frame<S>(socket: &mut S) -> serde_json::Value
where
    S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    loop {
        let frame = read_frame(socket).await;
        if frame["type"] != "status" {
            return frame;
        }
    }
}

/// Sends one well-formed chat frame naming the test model.
async fn send_chat<S>(socket: &mut S)
where
    S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
{
    let frame = serde_json::json!({
        "type": "chat",
        "model": "test-model",
        "messages": [{"role": "user", "content": "ping"}],
    })
    .to_string();
    socket
        .send(tungstenite::Message::Text(frame.into()))
        .await
        .expect("the chat frame is sent");
}

/// Sends one well-formed chat frame naming the test model, tagged with
/// `id`, through the typed client.
async fn send_chat_json(socket: &mut JsonSocket, id: u64) {
    socket
        .send_json(&json!({
            "type": "chat",
            "id": id,
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        }))
        .await;
}

/// Sends one well-formed chat frame naming the test model, tagged
/// with `id`.
async fn send_tagged_chat<S>(socket: &mut S, id: u64)
where
    S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
{
    let frame = serde_json::json!({
        "type": "chat",
        "id": id,
        "model": "test-model",
        "messages": [{"role": "user", "content": "ping"}],
    })
    .to_string();
    socket
        .send(tungstenite::Message::Text(frame.into()))
        .await
        .expect("the chat frame is sent");
}

/// Sends one cancel frame naming `id`.
async fn send_cancel<S>(socket: &mut S, id: u64)
where
    S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
{
    let frame = serde_json::json!({"type": "cancel", "id": id}).to_string();
    socket
        .send(tungstenite::Message::Text(frame.into()))
        .await
        .expect("the cancel frame is sent");
}

/// Reads non-status frames until `expected` terminal `done` frames
/// have arrived, returning every frame read, in order.
async fn replies_until_dones<S>(socket: &mut S, expected: usize) -> Vec<serde_json::Value>
where
    S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut replies = Vec::new();
        let mut settled = 0;
        while settled < expected {
            let frame = read_non_status_frame(socket).await;
            if frame["type"] == "done" {
                settled += 1;
            }
            replies.push(frame);
        }
        replies
    })
    .await
    .expect("every chat settles within the deadline")
}

/// Reads every event on the test's tape.
fn tape_events(tape_dir: &tempfile::TempDir) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
    raw.lines()
        .map(|line| serde_json::from_str(line).expect("the tape line is valid JSON"))
        .collect()
}

/// Polls the tape until it holds `expected` events, within a
/// deadline, and returns them.
async fn tape_events_when(tape_dir: &tempfile::TempDir, expected: usize) -> Vec<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(raw) = std::fs::read_to_string(tape_dir.path().join("tape.jsonl")) {
                let events: Vec<serde_json::Value> = raw
                    .lines()
                    .map(|line| serde_json::from_str(line).expect("the tape line is valid JSON"))
                    .collect();
                if events.len() >= expected {
                    break events;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the tape holds the expected events within the deadline")
}

/// Reads frames until `accept` holds, within a generous deadline,
/// returning every frame read - the accepted one last.
async fn frames_until<S>(
    socket: &mut S,
    accept: impl Fn(&serde_json::Value) -> bool,
) -> Vec<serde_json::Value>
where
    S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut frames = Vec::new();
        loop {
            let frame = read_frame(socket).await;
            let found = accept(&frame);
            frames.push(frame);
            if found {
                return frames;
            }
        }
    })
    .await
    .expect("the expected frame arrives within the deadline")
}
