//! THE PARITY GATE: in-process tests over the SSE mock gateway, each
//! pinned to a behavior the built-in `chat` agent must keep. The agent
//! replaced the direct-to-gateway chat relay; these tests hold the parity
//! the relay established.
//!
//! Every test launches the embedded `agents/chat.md`: the fixture's
//! agents directory does not exist, so what runs is exactly what ships -
//! a Markdown prompt on the unified runtime. A session's transcript sits
//! in memory until the harness's run log lands, so no gate here spans a
//! server restart; reconnect within one process is the agents suite's.

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
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt as _;
use serde_json::json;

use workshop_server::fixtures::{gateway_updater, replace_gateway, spawn_bindings_forwarder};
use workshop_server::{AgentsConfig, AppState, Config, InputResponse};

use crate::agents::{answer, collect_turn, delta_text, next_wait_token, wait_after};
use crate::common::{
    JsonSocket, connect_agents, echo_stream, launch, spawn_gateway, spawn_router, sse_chunk,
    sse_line, test_config, typed_catalog,
};

/// Every completion request body the gate mock received, in arrival
/// order: the gate's proof of exactly what the model was shown.
type CapturedRequests = Arc<Mutex<Vec<serde_json::Value>>>;

/// The gate mock: streams `echo:<last user message>` as a reasoning chunk
/// plus split content, echoing the requested model id back on every
/// chunk. Two message texts select failure shapes - `fail` is declined
/// with a 500, and `hang` opens the stream, sends one content chunk, and
/// never finishes. Every request body is captured for the history proofs.
fn gate_completions(captured: &CapturedRequests, body: &str) -> Response {
    let request: serde_json::Value = serde_json::from_str(body).expect("the request is JSON");
    captured
        .lock()
        .expect("the capture lock is healthy")
        .push(request.clone());
    let model = request["model"].as_str().unwrap_or("test-model").to_owned();
    let last = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the request includes a user message")
        .to_owned();
    if last == "fail" {
        return (StatusCode::INTERNAL_SERVER_ERROR, "injected model failure").into_response();
    }
    let null = serde_json::Value::Null;
    if last == "hang" {
        let opening = sse_line(&sse_chunk(&model, &json!({ "role": "assistant" }), &null))
            + &sse_line(&sse_chunk(&model, &json!({ "content": "nev" }), &null));
        let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(opening)])
            .chain(futures_util::stream::pending());
        return (
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(stream),
        )
            .into_response();
    }
    echo_stream(&model, &last)
}

/// A profile selection the gateway serves without a restart, whose
/// refreshed catalog replaces the launch-time model with `model-b`.
async fn switch_to_model_b() -> Response {
    axum::Json(json!({"profile": "beta", "restart_required": false})).into_response()
}

/// One workshop server over the gate mock. The agents directory is
/// missing on purpose: every `chat` launch runs the embedded built-in.
struct GateServer {
    /// The server's `ws://` base URL.
    ws_base: String,
    /// The shared state handle: menu, catalog, and session registry.
    state: AppState,
    /// The mock's captured request bodies.
    captured: CapturedRequests,
    /// Keeps the state directory alive.
    _dir: tempfile::TempDir,
}

/// Every id a gate may select, in the order the typed catalog lists
/// them. `model-b` stays first: the profile-switch gates rely on the menu
/// auto-selecting it from this list.
const GATE_MODELS: &[&str] = &["model-b", "model-a", "test-model", "claude-opus-4-6"];

/// Spawns the gate server with `models` in the retained catalog and the
/// first of them selected in the menu.
async fn spawn_chat_server(models: &[&str]) -> GateServer {
    spawn_chat_server_with_selection(models, models.first().copied()).await
}

/// Spawns the gate server with an explicit menu selection. `None` keeps
/// the catalog available to the launched agent while its live `ui()`
/// snapshot has no selected binding.
async fn spawn_chat_server_with_selection(models: &[&str], selected: Option<&str>) -> GateServer {
    let captured = CapturedRequests::default();
    let mock = Arc::clone(&captured);
    let gateway_url = spawn_gateway(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(move |body: String| {
                    let captured = Arc::clone(&mock);
                    async move { gate_completions(&captured, &body) }
                }),
            )
            .route("/admin/switch-profile", post(switch_to_model_b))
            .route(
                "/admin/profiles",
                get(|| async { axum::Json(json!({"profiles": ["main", "beta"]})) }),
            )
            .route(
                "/admin/status",
                get(|| async { axum::Json(json!({"profile": "beta"})) }),
            )
            .route("/v1/models", typed_catalog(GATE_MODELS)),
    )
    .await;
    let dir = tempfile::TempDir::new().expect("tempdir");
    let config = Config {
        agents: AgentsConfig {
            path: dir.path().join("missing-agents"),
        },
        ..test_config(&gateway_url, dir.path())
    };
    let (state, ws_base) = spawn_router(&config).await;
    // The router is bound without the serving loop that spawns the
    // registered tasks, so the forwarder that pushes gateway and catalog
    // replacements into the harness is spawned here.
    spawn_bindings_forwarder(&state);
    state.catalog().publish(
        models
            .iter()
            .map(|id| json!({ "id": id, "object": "model" }))
            .collect(),
    );
    if let Some(selected) = selected {
        state
            .menu()
            .set_selected(selected)
            .expect("the selected model is in the retained catalog");
    }
    GateServer {
        ws_base,
        state,
        captured,
        _dir: dir,
    }
}

/// Connects to `/agents/ws`, asserting the connect-time list is exactly
/// the built-in: end-to-end proof that a missing agents directory still
/// offers `chat`.
async fn connect_chat(base: &str) -> JsonSocket {
    connect_agents(base, &["chat"]).await
}

/// Launches the built-in chat and returns the session id.
async fn launch_chat(socket: &mut JsonSocket) -> String {
    launch(socket, "chat").await
}

/// Asserts that no input wait or error is buffered. The socket refuses
/// an unknown frame type inline, and its biased loop sends queued error
/// and wait frames before reading inbound, so any premature frame
/// arrives ahead of the refusal.
async fn assert_chat_quiet(socket: &mut JsonSocket) {
    socket.send_json(&json!({ "type": "quiet_probe" })).await;
    let frame = socket.recv_json().await;
    assert!(
        frame["type"] == "error"
            && frame["message"]
                .as_str()
                .is_some_and(|message| message.starts_with("unknown frame type")),
        "chat must stay dormant until a chat-capable catalog exists, got {frame}"
    );
}

/// The `(role, content)` pairs of one captured request's message list.
fn role_content_pairs(request: &serde_json::Value) -> Vec<(String, String)> {
    request["messages"]
        .as_array()
        .expect("a captured request includes a messages array")
        .iter()
        .map(|message| {
            (
                message["role"]
                    .as_str()
                    .expect("every message has a role")
                    .to_owned(),
                message["content"]
                    .as_str()
                    .expect("every message has string content")
                    .to_owned(),
            )
        })
        .collect()
}

/// Builds one owned `(role, content)` pair for the assertions.
fn pair(role: &str, content: &str) -> (String, String) {
    (role.to_owned(), content.to_owned())
}

mod canonical_sequence;
mod lifecycle;
mod overload;
mod protocol;
mod recovery;
