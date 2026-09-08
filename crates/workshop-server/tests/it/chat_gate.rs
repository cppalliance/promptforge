//! THE PARITY GATE: seven in-process tests over the SSE mock gateway, each
//! pinned to a behavior the built-in `chat` agent must keep. The agent
//! replaced the direct-to-gateway chat relay; these tests hold the parity
//! the relay established.
//!
//! Every test launches the embedded `agents/chat.lua`: the fixture's
//! agents directory does not exist, so what runs is exactly what ships.

// clippy.toml's allow-expect-in-tests covers #[test] functions only, not
// the helpers they share; failing a test by panicking with the invariant
// named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt as _;
use serde_json::json;
use tokio::sync::broadcast;

use promptforge_agent::{AgentConfig, AgentError, AgentLimits, run_agent_with_client};
use promptforge_core_support::cancel::CancelHandle;
use promptforge_core_support::events::{EventLog as _, RuntimeEventKind};
use promptforge_core_support::observe::Observer;
use promptforge_model_client::client::{
    GatewayClient as ModelClient, GatewayEndpoint, SecretString,
};
use promptforge_model_client::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use promptforge_store::StoreRef;
use promptforge_tools::{Tool, ToolCatalog};
use workshop_server::fixtures::{gateway_updater, replace_gateway, state_with_gateway};
use workshop_server::{
    AgentsConfig, AppState, Config, GatewayConfig, InputFrame, InputResponse, ResolvedGateway,
    ServerConfig, UserInputTool, WaitRegistry, WorkshopObserver, deliver_input_response, router,
};

use crate::agents::{answer, collect_turn, delta_text, next_wait_token, wait_after};
use crate::common::{JsonSocket, spawn_gateway};

/// Every completion request body the gate mock received, in arrival
/// order: the gate's proof of exactly what the model was shown.
type CapturedRequests = Arc<Mutex<Vec<serde_json::Value>>>;

/// One SSE data line carrying `event`.
fn sse_line(event: &serde_json::Value) -> String {
    format!("data: {event}\n\n")
}

/// One OpenAI-shaped streaming chunk attributed to `model`.
fn sse_chunk(
    model: &str,
    delta: &serde_json::Value,
    finish: &serde_json::Value,
) -> serde_json::Value {
    json!({
        "model": model,
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
    })
}

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
        .expect("the request carries a user message")
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
    let reply = format!("echo:{last}");
    let (first, second) = reply.split_at(reply.len() / 2);
    let mut sse = String::new();
    for event in [
        sse_chunk(&model, &json!({ "role": "assistant" }), &null),
        sse_chunk(&model, &json!({ "reasoning_content": "mm" }), &null),
        sse_chunk(&model, &json!({ "content": first }), &null),
        sse_chunk(&model, &json!({ "content": second }), &null),
        sse_chunk(&model, &json!({}), &json!("stop")),
    ] {
        sse.push_str(&sse_line(&event));
    }
    sse.push_str("data: [DONE]\n\n");
    ([(header::CONTENT_TYPE, "text/event-stream")], sse).into_response()
}

/// A successful profile switch whose refreshed catalog replaces the
/// launch-time model with `model-b`.
async fn switch_to_model_b() -> Response {
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        concat!(
            "data: {\"stage\":\"loading-profile\"}\n\n",
            "data: {\"stage\":\"stopping-models\"}\n\n",
            "data: {\"stage\":\"starting-models\"}\n\n",
            "data: {\"status\":\"ready\",\"profile\":\"beta\"}\n\n",
        ),
    )
        .into_response()
}

/// One workshop server over the gate mock. The agents directory is
/// missing on purpose: every `chat` launch runs the embedded built-in.
struct GateServer {
    /// The server's `ws://` base URL.
    ws_base: String,
    /// The mock gateway's `http://` base URL, for the restart relaunch.
    gateway_url: String,
    /// The shared state handle: menu, catalog, and session registry.
    state: AppState,
    /// The mock's captured request bodies.
    captured: CapturedRequests,
    /// Keeps the state directory (and its session JSONLs) alive.
    dir: tempfile::TempDir,
}

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
            .route(
                "/v1/models",
                get(|| async {
                    axum::Json(json!({
                        "object": "list",
                        "data": [{"id": "model-b", "object": "model"}],
                    }))
                }),
            ),
    )
    .await;
    let dir = tempfile::TempDir::new().expect("tempdir");
    let config = Config {
        gateway: GatewayConfig {
            base_url: gateway_url.clone(),
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            state_dir: dir.path().to_path_buf(),
            ..ServerConfig::default()
        },
        agents: AgentsConfig {
            path: dir.path().join("missing-agents"),
        },
    };
    // Discovery is bypassed: a test never consults the real run directory.
    let gateway = ResolvedGateway::from_config(&config.gateway);
    let state = state_with_gateway(&config, &gateway).expect("state builds in tests");
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the gate test server");
    let addr = listener.local_addr().expect("gate test server address");
    let served = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, router(served))
            .await
            .expect("gate test server serves");
    });
    GateServer {
        ws_base: format!("ws://{addr}"),
        gateway_url,
        state,
        captured,
        dir,
    }
}

/// Connects to `/agents/ws`, asserting the connect-time list is exactly
/// the built-in: end-to-end proof that a missing agents directory still
/// offers `chat`.
async fn connect_chat(base: &str) -> JsonSocket {
    let mut socket = JsonSocket::connect(&format!("{base}/agents/ws")).await;
    assert_eq!(
        socket.recv_json().await,
        json!({ "type": "agents", "agents": ["chat"] }),
        "a missing agents directory still offers the built-in chat"
    );
    socket
}

/// Launches the built-in chat and returns the session id.
async fn launch_chat(socket: &mut JsonSocket) -> String {
    socket
        .send_json(&json!({ "type": "launch", "agent": "chat" }))
        .await;
    let frame = socket.recv_json().await;
    assert_eq!(
        frame["type"], "agent_session",
        "launch acknowledged: {frame}"
    );
    assert_eq!(frame["agent"], "chat");
    frame["session"]
        .as_str()
        .expect("the acknowledgment carries the session id")
        .to_owned()
}

/// Asserts that no input wait or error arrives during `duration`.
async fn assert_chat_quiet(socket: &mut JsonSocket, duration: Duration) {
    let frame = tokio::time::timeout(duration, socket.recv_json()).await;
    assert!(
        frame.is_err(),
        "chat must stay dormant until a chat-capable catalog exists, got {frame:?}"
    );
}

/// The `(role, content)` pairs of one captured request's message list.
fn role_content_pairs(request: &serde_json::Value) -> Vec<(String, String)> {
    request["messages"]
        .as_array()
        .expect("a captured request carries a messages array")
        .iter()
        .map(|message| {
            (
                message["role"]
                    .as_str()
                    .expect("every message has a role")
                    .to_owned(),
                message["content"]
                    .as_str()
                    .expect("every message carries string content")
                    .to_owned(),
            )
        })
        .collect()
}

/// Builds one owned `(role, content)` pair for the assertions.
fn pair(role: &str, content: &str) -> (String, String) {
    (role.to_owned(), content.to_owned())
}

/// The running relaunch of the restart gate: everything the test drives
/// and tears down.
struct RestoredChat {
    /// Announces the relaunched agent's input waits.
    frames: broadcast::Receiver<InputFrame>,
    /// The registry the response delivery completes waits through.
    waits: Arc<WaitRegistry>,
    /// Ends the relaunched run at teardown.
    cancel: CancelHandle,
    /// The run task, joined at teardown.
    run: tokio::task::JoinHandle<Result<(), AgentError>>,
}

/// The relaunch half of the restart gate: the supervisor's own pieces -
/// the `user_input` tool over a fresh wait registry, the embedded chat
/// source, and a client aimed at the mock gateway - spawned over the
/// restored log.
fn spawn_restored_chat(
    restored: &Arc<WorkshopObserver>,
    session: &str,
    gateway_url: &str,
) -> RestoredChat {
    let waits = Arc::new(WaitRegistry::new());
    let (frames_tx, frames) = broadcast::channel(8);
    let tool: Arc<dyn Tool> = Arc::new(UserInputTool::new(Arc::clone(&waits), frames_tx));
    let tools = ToolCatalog::new(&[tool]).expect("the relaunch tool catalog builds");
    let context = NonZeroU32::new(8192).expect("8192 is non-zero");
    let models = ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("test-model").expect("the test model name is valid"),
        "the gate's mock model",
        context,
        ThinkingMode::Never,
    )])
    .expect("the relaunch model catalog builds");
    let client = ModelClient::new(
        GatewayEndpoint::new(&format!("{gateway_url}/v1")).expect("the mock endpoint parses"),
        SecretString::new("test-key").expect("the test key is non-empty"),
    );
    let cancel = CancelHandle::new();
    let config = AgentConfig {
        name: "chat".to_owned(),
        execution: session.to_owned(),
        observer: Arc::clone(restored) as Arc<dyn Observer>,
        cancel: cancel.clone(),
        event_log: Some(Arc::clone(restored) as _),
        on_delta: None,
        ui: Some(Arc::new(
            || json!({ "selected_model": "test-model", "workspace_root": serde_json::Value::Null }),
        )),
        limits: AgentLimits::default(),
    };
    let source = include_str!("../../agents/chat.lua");
    let run = tokio::spawn(async move {
        let store = StoreRef::memory();
        run_agent_with_client(source, &tools, &models, &store, config, Some(client)).await
    });
    RestoredChat {
        frames,
        waits,
        cancel,
        run,
    }
}

include!("chat_gate/protocol.rs");
include!("chat_gate/lifecycle.rs");
include!("chat_gate/recovery.rs");
include!("chat_gate/overload.rs");
include!("chat_gate/canonical_sequence.rs");
