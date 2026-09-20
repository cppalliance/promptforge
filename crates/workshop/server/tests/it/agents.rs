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
    state_with_gateway,
};
use workshop_server::{
    AgentsConfig, AppState, Config, GatewayConfig, ResolvedGateway, ServerConfig, router,
};

use crate::common::{JsonSocket, spawn_gateway};

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

/// Streams `echo:<last user message>` as an SSE completion: a reasoning
/// chunk, the content split across two chunks, the finish chunk, and the
/// `[DONE]` sentinel - so a turn provably yields multiple live deltas.
async fn echo_completions(body: String) -> Response {
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    let text = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the request carries a user message");
    let reply = format!("echo:{text}");
    let (first, second) = reply.split_at(reply.len() / 2);
    let chunk = |delta: serde_json::Value, finish: serde_json::Value| {
        json!({
            "model": "test-model",
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        })
        .to_string()
    };
    let events = [
        chunk(json!({ "role": "assistant" }), serde_json::Value::Null),
        chunk(
            json!({ "reasoning_content": "mm" }),
            serde_json::Value::Null,
        ),
        chunk(json!({ "content": first }), serde_json::Value::Null),
        chunk(json!({ "content": second }), serde_json::Value::Null),
        chunk(json!({}), json!("stop")),
    ];
    let mut sse = String::new();
    for event in events {
        sse.push_str("data: ");
        sse.push_str(&event);
        sse.push_str("\n\n");
    }
    sse.push_str("data: [DONE]\n\n");
    ([(header::CONTENT_TYPE, "text/event-stream")], sse).into_response()
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

/// Adds the typed `/v1/models` catalog a launch resolves the menu selection
/// through. Every id these tests select carries a window that clears the
/// built-in chat's declared minimum; a mock without this route fails the
/// launch with the reported catalog-fetch cause.
fn with_typed_catalog(router: Router) -> Router {
    router.route(
        "/v1/models",
        axum::routing::get(|| async {
            let entry = |id: &str| {
                json!({
                    "id": id, "object": "model", "kind": "chat", "description": id,
                    "context": 200_000, "thinking": "never",
                })
            };
            axum::Json(json!({
                "object": "list",
                "data": [
                    entry("model-a"),
                    entry("model-b"),
                    entry("model-c"),
                    entry("test-model"),
                ],
            }))
        }),
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
/// relaunched chat run starts a new message list, because history lives in
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
        .expect("the request carries a messages array");
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
        gateway: GatewayConfig {
            base_url,
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            state_dir: dir.path().to_path_buf(),
            ..ServerConfig::default()
        },
        agents: AgentsConfig { path: agents_dir },
    };
    // Discovery is bypassed: a test never consults the real run directory.
    let gateway = ResolvedGateway::from_config(&config.gateway);
    let state = state_with_gateway(&config, &gateway).expect("state builds in tests");
    // The router is bound directly, without the serving loop that spawns
    // the registered tasks, so the forwarder that pushes gateway and
    // catalog replacements into the harness is spawned here.
    spawn_bindings_forwarder(&state);
    // The session's model catalog is built from the retained catalog at
    // launch, so the catalog lands before any test launches.
    state
        .catalog()
        .publish(vec![json!({ "id": "test-model", "object": "model" })]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the agent test server");
    let addr = listener.local_addr().expect("agent test server address");
    let served = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, router(served))
            .await
            .expect("agent test server serves");
    });
    (format!("ws://{addr}"), dir, state)
}

/// Publishes `base_url` as the next complete Gateway generation.
fn replace_gateway(state: &AppState, base_url: &str, _epoch: u64) {
    replace_fixture_gateway(&gateway_updater(state), base_url, "replacement-key")
        .expect("the replacement Gateway publishes");
}

/// Connects to `/agents/ws` and consumes the connect-time agent list.
async fn connect(base: &str) -> JsonSocket {
    let mut socket = JsonSocket::connect(&format!("{base}/agents/ws")).await;
    assert_eq!(
        socket.recv_json().await,
        json!({ "type": "agents", "agents": ["chat", "echo"] }),
        "the connect-time push lists the discovered agents plus the built-in chat"
    );
    socket
}

/// Launches the echo agent on `socket` and returns the session id from
/// the acknowledgment frame.
async fn launch_echo(socket: &mut JsonSocket) -> String {
    launch_agent(socket, "echo").await
}

/// Launches `agent` and returns its acknowledged session id.
async fn launch_agent(socket: &mut JsonSocket, agent: &str) -> String {
    socket
        .send_json(&json!({ "type": "launch", "agent": agent }))
        .await;
    let frame = socket.recv_json().await;
    assert_eq!(frame["type"], "agent_session");
    assert_eq!(frame["agent"], agent);
    frame["session"]
        .as_str()
        .expect("the acknowledgment carries the session id")
        .to_owned()
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
mod turns;
