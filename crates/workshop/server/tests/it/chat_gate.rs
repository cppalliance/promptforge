//! THE PARITY GATE: in-process tests over the SSE mock gateway, each
//! pinned to a behavior the built-in `chat` agent must keep. The agent
//! replaced the direct-to-gateway chat relay; these tests hold the parity
//! the relay established.
//!
//! Every test launches the embedded `agents/chat.md`: the fixture's
//! agents directory does not exist, so what runs is exactly what ships -
//! a Markdown prompt on the unified runtime.

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
use tokio::sync::broadcast;

use promptforge_api_runtime::client::{
    GatewayClient as ModelClient, GatewayEndpoint, SecretString,
};
use promptforge_api_runtime::{Prompt, RunContext, RunResult};
use promptforge_api_types::cancel::CancelHandle;
use promptforge_api_types::events::{EventLog as _, RuntimeEventKind};
use promptforge_api_types::models::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge_api_types::observe::Observer;
use workshop_server::fixtures::{gateway_updater, replace_gateway, state_with_gateway};
use workshop_server::{
    AgentsConfig, AppState, Config, GatewayConfig, InputFrame, InputResponse, ResolvedGateway,
    ServerConfig, SessionInputBroker, WaitRegistry, WorkshopObserver, router,
};

use crate::agents::{answer, collect_turn, delta_text, next_wait_token, wait_after};
use crate::common::{JsonSocket, spawn_gateway};

/// The embedded built-in chat prompt, exactly what a `chat` launch runs.
const CHAT_MD: &str = include_str!("../../../sessions/agents/chat.md");

/// The relaunch harness's terminal outcome, mirroring the supervisor's
/// `AgentRunError`: cancellation maps to `Interrupted`, and every other
/// run failure carries its rendered message.
#[derive(Debug)]
enum AgentError {
    /// The run's cancel handle fired.
    Interrupted,
    /// The prompt run failed.
    Program {
        /// The failure's rendered message.
        message: String,
    },
}

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
    /// The mock gateway's `http://` base URL, for the restart relaunch.
    gateway_url: String,
    /// The shared state handle: menu, catalog, and session registry.
    state: AppState,
    /// The mock's captured request bodies.
    captured: CapturedRequests,
    /// Keeps the state directory (and its session JSONLs) alive.
    dir: tempfile::TempDir,
}

/// The typed catalog a mock gateway serves from `/v1/models`: the launch
/// resolves the menu selection through it, so every id a gate may select
/// carries a window that clears chat's declared minimum, and a gate tests
/// the host wiring rather than a refused binding. `model-b` stays first:
/// the profile-switch gates rely on the menu auto-selecting it from this
/// list.
async fn gate_models() -> axum::Json<serde_json::Value> {
    let entry = |id: &str| {
        json!({
            "id": id, "object": "model", "kind": "chat", "description": id,
            "context": 200_000, "thinking": "never",
        })
    };
    axum::Json(json!({
        "object": "list",
        "data": [
            entry("model-b"),
            entry("model-a"),
            entry("test-model"),
            entry("claude-opus-4-6"),
        ],
    }))
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
            .route("/v1/models", get(gate_models)),
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
/// the session's wait registry behind the generic input broker, the
/// embedded chat prompt, the shared session environment carrying the
/// first-party capabilities, and a client aimed at the mock gateway - run
/// on the unified runtime over the restored log. The context carries the
/// current model directly: the supervisor resolves the dropdown's
/// selection at launch, and this harness drives the run beneath that
/// seam.
fn spawn_restored_chat(
    restored: &Arc<WorkshopObserver>,
    session: &str,
    gateway_url: &str,
) -> RestoredChat {
    let waits = Arc::new(WaitRegistry::new());
    let (frames_tx, frames) = broadcast::channel(8);
    let broker = Arc::new(SessionInputBroker::new(Arc::clone(&waits), frames_tx));
    let client = ModelClient::new(
        GatewayEndpoint::new(&format!("{gateway_url}/v1")).expect("the mock endpoint parses"),
        SecretString::new("test-key").expect("the test key is non-empty"),
    );
    let cancel = CancelHandle::new();
    let observer: Arc<dyn Observer> = restored.clone();
    let env = workshop_sessions::session_environment(gateway_url, "test-key")
        .expect("the mock gateway shape builds the session environment");
    let model = ModelDescriptor::new(
        ModelId::gateway("test-model").expect("the test model id is valid"),
        "test model",
        std::num::NonZeroU32::new(200_000).expect("200000 is non-zero"),
        ThinkingMode::Never,
    );
    let ctx = RunContext::new(session.to_owned())
        .observer(Arc::clone(&observer))
        .client(client)
        .cancel(cancel.clone())
        .input_broker(broker)
        .model(model);
    let execution = session.to_owned();
    let run = tokio::spawn(async move {
        let result = async {
            let prompt = Prompt::parse(CHAT_MD, &execution, observer.as_ref())
                .expect("the embedded chat prompt parses");
            env.run(&prompt, "", ctx).await
        }
        .await;
        match result {
            RunResult::Ok(_output) => Ok(()),
            RunResult::Cancelled => Err(AgentError::Interrupted),
            RunResult::Failure(error) => Err(AgentError::Program {
                message: error.to_string(),
            }),
        }
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
