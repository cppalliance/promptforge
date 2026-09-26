//! End-to-end agent-session tests over the `/agents/ws` socket: launch,
//! the full turn cycle with reply-id coalescing and indexed durable
//! frames, reconnect replay, turn-cancel, session isolation, status-bus
//! order, backoff reset, and teardown wait cleanup - all in-process
//! against an SSE mock gateway.

// clippy.toml's allow-expect-in-tests covers #[test] functions only, not
// the helpers they share; failing a test by panicking with the invariant
// named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::json;
use tokio::sync::Notify;

use workshop_server::fixtures::{
    gateway_updater, replace_gateway as replace_fixture_gateway, spawn_bindings_forwarder,
};
use workshop_server::{AgentsConfig, AppState, Config};

use crate::common::{
    JsonSocket, connect_agents, echo_stream, launch, spawn_gateway, spawn_router, test_config,
    typed_catalog,
};

/// The echo agent: a Markdown prompt on the unified runtime that loops
/// on `user_input`, runs one chat round per input against the fixture's
/// `test-model`, and returns on `quit`.
const ECHO_MD: &str = r"---
name: echo
description: The echo test agent on the unified runtime.
promptforge: 0
---

# Echo

## Conversation

```lua
local history = messages.new()
while true do
    local text, available = user_input()
    if not available then
        return
    end
    if text == 'quit' then
        return
    end
    history:user(text)
    models.loop(models.get('test-model'), history)
end
```
";

/// Streams `echo:<last user message>` from `test-model` as an SSE
/// completion.
async fn echo_completions(body: String) -> Response {
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    let text = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the request includes a user message");
    echo_stream("test-model", text)
}

/// Accepts one completion and then leaves its SSE body open forever.
fn hanging_completions(started: &Notify) -> Response {
    started.notify_one();
    let stream = futures_util::stream::pending::<Result<String, std::io::Error>>();
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}

/// Adds the typed `/v1/models` catalog holding every id these tests
/// select; a mock without this route fails the launch with the reported
/// catalog-fetch cause.
fn with_typed_catalog(router: Router) -> Router {
    router.route(
        "/v1/models",
        typed_catalog(&["model-a", "model-b", "model-c", "test-model"]),
    )
}

/// Records one completion body for endpoint and binding assertions.
fn record_request(requests: &Mutex<Vec<serde_json::Value>>, body: &str) {
    requests
        .lock()
        .expect("the request capture lock is healthy")
        .push(serde_json::from_str(body).expect("the request is JSON"));
}

/// Asserts one replacement request and its fresh-history boundary: the
/// relaunched chat run starts a new message list, because history sits in
/// the section's Lua state until the deferred persistence work lands.
fn assert_replacement_request(
    requests: &Mutex<Vec<serde_json::Value>>,
    model: &str,
    current_input: &str,
) {
    let requests = requests
        .lock()
        .expect("the request capture lock is healthy");
    assert_eq!(requests.len(), 1, "one replacement run dispatches");
    assert_eq!(
        requests[0]["model"], model,
        "the replacement request reads the live selection"
    );
    let messages = requests[0]["messages"]
        .as_array()
        .expect("the request includes a messages array");
    assert_eq!(
        messages.len(),
        1,
        "the relaunched run starts a fresh message list"
    );
    assert_eq!(
        messages[0]["content"], current_input,
        "the fresh list opens with the new turn's input"
    );
}

/// Binds the workshop router against an echoing SSE mock gateway, with
/// one discovered agent (`echo`) and the retained catalog already
/// holding `test-model`. Returns the server's base `ws://` URL, the
/// tempdir keeping the state alive, and the shared state handle.
async fn spawn_agent_server() -> (String, tempfile::TempDir, AppState) {
    let base_url = spawn_gateway(with_typed_catalog(
        Router::new().route("/v1/chat/completions", post(echo_completions)),
    ))
    .await;
    spawn_agent_server_for_gateway(base_url).await
}

/// Binds the workshop router to an injected Gateway endpoint.
async fn spawn_agent_server_for_gateway(base_url: String) -> (String, tempfile::TempDir, AppState) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let agents_dir = dir.path().join("agents");
    std::fs::create_dir(&agents_dir).expect("the agents directory creates");
    std::fs::write(agents_dir.join("echo.md"), ECHO_MD).expect("the echo agent writes");
    let config = Config {
        agents: AgentsConfig { path: agents_dir },
        ..test_config(&base_url, dir.path())
    };
    let (state, base) = spawn_router(&config).await;
    // The router is bound without the serving loop that spawns the
    // registered tasks, so the forwarder that pushes gateway and catalog
    // replacements into the harness is spawned here.
    spawn_bindings_forwarder(&state);
    // The session's model catalog is built from the retained catalog at
    // launch, so the catalog lands before any test launches.
    state
        .catalog()
        .publish(vec![json!({ "id": "test-model", "object": "model" })]);
    (base, dir, state)
}

/// Publishes `base_url` as the next complete Gateway generation.
fn replace_gateway(state: &AppState, base_url: &str, _epoch: u64) {
    replace_fixture_gateway(&gateway_updater(state), base_url, "replacement-key")
        .expect("the replacement Gateway publishes");
}

/// Connects to `/agents/ws`, where the connect-time push lists the
/// discovered agents plus the built-in chat.
async fn connect(base: &str) -> JsonSocket {
    connect_agents(base, &["chat", "echo"]).await
}

/// Launches the echo agent on `socket` and returns its session id.
async fn launch_echo(socket: &mut JsonSocket) -> String {
    launch(socket, "echo").await
}

/// Receives frames until the next `input_required` and returns its
/// token, asserting no error frame slips through on the way.
pub(crate) async fn next_wait_token(socket: &mut JsonSocket) -> String {
    let frame = socket
        .recv_until(Duration::from_secs(10), |frame| {
            assert_ne!(
                frame["type"], "error",
                "no error frame may interrupt: {frame}"
            );
            frame["type"] == "input_required"
        })
        .await;
    frame["token"]
        .as_str()
        .expect("the wait announces its token")
        .to_owned()
}

/// Answers the wait holding `token` with `text`.
pub(crate) async fn answer(socket: &mut JsonSocket, token: &str, text: &str) {
    socket
        .send_json(&json!({ "type": "input_response", "token": token, "text": text }))
        .await;
}

/// Everything one turn produced, collected until its completed reply
/// event: the delta frames, the durable event frames, and any wait
/// tokens announced along the way (the next turn's `input_required` may
/// hit the wire before the reply's own event frame - frame families
/// promise order within themselves, not across each other).
pub(crate) struct Turn {
    pub(crate) deltas: Vec<serde_json::Value>,
    pub(crate) events: Vec<serde_json::Value>,
    pub(crate) waits: Vec<String>,
}

/// Collects frames until the turn's `agent_message` event arrives,
/// splitting deltas, durable events, and announced waits, and refusing
/// error frames.
pub(crate) async fn collect_turn(socket: &mut JsonSocket) -> Turn {
    let mut deltas = Vec::new();
    let mut events = Vec::new();
    let mut waits = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = socket.recv_json().await;
            match frame["type"].as_str() {
                Some("agent_delta") => deltas.push(frame),
                Some("agent_event") => {
                    let done = frame["event"]["kind"] == "agent_message";
                    events.push(frame);
                    if done {
                        break;
                    }
                }
                Some("input_required") => waits.push(
                    frame["token"]
                        .as_str()
                        .expect("the wait announces its token")
                        .to_owned(),
                ),
                Some("error") => panic!("no error frame may interrupt a turn: {frame}"),
                // Status frames interleave freely.
                _ => {}
            }
        }
    })
    .await
    .expect("the turn completes within the deadline");
    Turn {
        deltas,
        events,
        waits,
    }
}

/// The wait token following `turn`: one already captured during the
/// collection, else the next announced on the socket.
pub(crate) async fn wait_after(socket: &mut JsonSocket, turn: &Turn) -> String {
    match turn.waits.first() {
        Some(token) => token.clone(),
        None => next_wait_token(socket).await,
    }
}

/// Concatenates the turn's text-delta contents.
pub(crate) fn delta_text(turn: &Turn) -> String {
    turn.deltas
        .iter()
        .filter(|delta| delta["kind"] == "text")
        .filter_map(|delta| delta["content"].as_str())
        .collect()
}

mod lifecycle;
mod refusals;
mod replacement;
mod revoke;
mod turns;
