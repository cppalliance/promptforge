//! THE PARITY GATE: six in-process tests over the SSE mock gateway, each
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
use axum::routing::post;
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
use workshop_server::{
    AgentsConfig, AppState, Config, GatewayConfig, InputFrame, InputResponse, ServerConfig,
    UserInputTool, WaitRegistry, WorkshopObserver, deliver_input_response, router,
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
    let captured = CapturedRequests::default();
    let mock = Arc::clone(&captured);
    let gateway_url = spawn_gateway(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let captured = Arc::clone(&mock);
            async move { gate_completions(&captured, &body) }
        }),
    ))
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
    let state = AppState::new(&config).expect("state builds in tests");
    state.catalog().publish(
        models
            .iter()
            .map(|id| json!({ "id": id, "object": "model" }))
            .collect(),
    );
    state
        .menu()
        .set_selected(models[0])
        .expect("the first model is in the retained catalog");
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

/// GATE 1 - multi-turn history. Current-chat behavior: the conversation
/// accumulates turn over turn, and what the user typed reaches the model
/// byte-exact with no untrusted envelope around it.
#[tokio::test]
async fn gate_history_accumulates_across_three_turns_byte_exact() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let gnarly = "line1\r\nline2 \"quoted\" {\"text\":\"decoy\"} \\slash 🦀";
    let inputs = ["first ping", gnarly, "third"];
    let mut token = next_wait_token(&mut socket).await;
    for input in inputs {
        answer(&mut socket, &token, input).await;
        let turn = collect_turn(&mut socket).await;
        assert_eq!(delta_text(&turn), format!("echo:{input}"));
        token = wait_after(&mut socket, &turn).await;
    }

    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests.len(), 3, "three turns are three model rounds");
        assert_eq!(
            role_content_pairs(&requests[0]),
            vec![pair("user", "first ping")],
            "the first round carries exactly the first input"
        );
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![
                pair("user", "first ping"),
                pair("assistant", "echo:first ping"),
                pair("user", gnarly),
            ],
            "the second round carries the first exchange plus the new input, \
             the gnarly user text byte-exact and envelope-free"
        );
        assert_eq!(
            role_content_pairs(&requests[2]),
            vec![
                pair("user", "first ping"),
                pair("assistant", "echo:first ping"),
                pair("user", gnarly),
                pair("assistant", &format!("echo:{gnarly}")),
                pair("user", "third"),
            ],
            "the third round carries the whole accumulated conversation"
        );
    }
    socket.close().await;
}

/// GATE 2 - live streaming. Current-chat behavior: while the model
/// generates, the client sees answer text and reasoning arrive as live
/// chunks, and the completed reply supersedes them under the same id.
#[tokio::test]
async fn gate_streaming_delivers_text_and_reasoning_deltas_then_the_reply() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let turn = collect_turn(&mut socket).await;

    let reasoning: String = turn
        .deltas
        .iter()
        .filter(|delta| delta["kind"] == "reasoning")
        .filter_map(|delta| delta["content"].as_str())
        .collect();
    assert_eq!(
        reasoning, "mm",
        "reasoning streams live on its own side channel during generation"
    );
    assert!(
        turn.deltas
            .iter()
            .filter(|delta| delta["kind"] == "text")
            .count()
            >= 2,
        "the mock splits content, so generation provably streams in chunks"
    );
    assert_eq!(
        delta_text(&turn),
        "echo:ping",
        "the live text chunks assemble the reply"
    );

    let reply = turn
        .events
        .last()
        .expect("the turn ends with its reply event");
    assert_eq!(reply["event"]["kind"], "agent_message");
    assert_eq!(
        reply["event"]["content"], "echo:ping",
        "the completed reply arrives after the deltas it supersedes"
    );
    assert!(
        turn.deltas
            .iter()
            .all(|delta| delta["reply"] == reply["reply"]),
        "deltas and the completed reply share the superseding id"
    );
    socket.close().await;
}

/// GATE 3 - model switch. Current-chat behavior: selecting another model
/// takes effect on the next turn, and the reply is attributed to the
/// model that produced it.
#[tokio::test]
async fn gate_model_switch_takes_effect_next_turn_with_attribution() {
    let server = spawn_chat_server(&["model-a", "model-b"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "one").await;
    let turn = collect_turn(&mut socket).await;
    let reply = turn.events.last().expect("the first turn completes");
    assert_eq!(
        reply["event"]["model"], "model-a",
        "the first turn runs on the selected model"
    );

    server
        .state
        .menu()
        .set_selected("model-b")
        .expect("model-b is in the retained catalog");

    let token = wait_after(&mut socket, &turn).await;
    answer(&mut socket, &token, "two").await;
    let turn = collect_turn(&mut socket).await;
    let reply = turn.events.last().expect("the second turn completes");
    assert_eq!(
        reply["event"]["model"], "model-b",
        "the switch takes effect next turn; the reply event carries the new model id"
    );
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests[0]["model"], "model-a");
        assert_eq!(
            requests[1]["model"], "model-b",
            "the request itself names the newly selected model"
        );
    }
    socket.close().await;
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

/// GATE 4 - restart. Current-chat behavior it replaces: a conversation
/// does not die with its process. The persisted JSONL alone restores it,
/// and the relaunched agent resumes waiting for input - the supervisor's
/// own relaunch shape driven with the log reloaded from disk.
#[tokio::test]
async fn gate_restart_reloads_the_jsonl_and_resumes_waiting_for_input() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let live = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&live), "echo:ping");
    socket.close().await;
    assert!(
        server.state.agents().close(&session),
        "the session ends; only the JSONL survives"
    );

    let log_path = server
        .dir
        .path()
        .join("sessions")
        .join(format!("{session}.jsonl"));
    let restored =
        Arc::new(WorkshopObserver::load_from(&log_path).expect("the persisted JSONL reloads"));
    assert_eq!(
        restored.len(),
        4,
        "the whole conversation restores: input, tool result, thinking, reply"
    );
    assert_eq!(
        restored.get(0).map(|event| event.content),
        Some("ping".to_owned())
    );
    assert_eq!(
        restored.get(3).map(|event| event.content),
        Some("echo:ping".to_owned())
    );

    let mut relaunch = spawn_restored_chat(&restored, &session, &server.gateway_url);

    // The relaunched agent resumes waiting: its first act is user_input.
    let frame = tokio::time::timeout(Duration::from_secs(10), relaunch.frames.recv())
        .await
        .expect("the relaunched agent asks for input")
        .expect("the frames channel is open");
    let InputFrame::Required { token } = frame else {
        panic!("the relaunched agent must open a wait, got {frame:?}");
    };

    // Answering proves the conversation itself was restored: the next
    // round shows the model the old exchange plus the new input.
    let mut entries = restored.subscribe();
    deliver_input_response(
        restored.as_ref(),
        &relaunch.waits,
        &session,
        "chat",
        InputResponse {
            token,
            text: "and back".to_owned(),
        },
    )
    .expect("the wait completes");
    let reply = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = entries.recv().await.expect("the log broadcast stays open");
            if event.kind == RuntimeEventKind::AssistantReply {
                break event;
            }
        }
    })
    .await
    .expect("the restarted agent completes a round");
    assert_eq!(reply.content, "echo:and back");
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![
                pair("user", "ping"),
                pair("assistant", "echo:ping"),
                pair("user", "and back"),
            ],
            "the reloaded JSONL alone rebuilt the conversation the model sees"
        );
    }

    // Teardown: the loop is back on user_input; cancellation ends it.
    relaunch.cancel.cancel();
    let result = relaunch.run.await.expect("the relaunched run joins");
    assert!(
        matches!(result, Err(AgentError::Interrupted)),
        "cancellation ends the relaunched run cleanly, got {result:?}"
    );
}

/// GATE 5 - turn-cancel. Current-chat behavior: the stop button kills
/// generation mid-stream without an error, and the chat is immediately
/// usable again.
#[tokio::test]
async fn gate_cancel_mid_generation_returns_to_waiting_and_next_input_works() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "hang").await;
    // Generation is provably live: a text chunk of the never-finishing
    // stream has reached the wire.
    let delta = socket
        .recv_until(Duration::from_secs(10), |frame| {
            assert_ne!(
                frame["type"], "error",
                "the hanging turn is not an error: {frame}"
            );
            frame["type"] == "agent_delta" && frame["kind"] == "text"
        })
        .await;
    assert_eq!(delta["content"], "nev");

    socket.send_json(&json!({ "type": "cancel" })).await;

    // Cancellation is a stop reason: the relaunched run returns to
    // waiting, and next_wait_token refuses error frames on the way -
    // which asserts exactly the no-error contract.
    let fresh = next_wait_token(&mut socket).await;
    assert_ne!(fresh, token, "the relaunched run opens a fresh wait");
    answer(&mut socket, &fresh, "after cancel").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:after cancel",
        "the next input after a mid-generation cancel runs a full turn"
    );
    assert!(
        turn.events
            .iter()
            .all(|event| event["event"]["content"] != "echo:hang"),
        "the cancelled generation never completes into a reply"
    );
    socket.close().await;
}

/// GATE 6 - error survival. Current-chat behavior: a failed completion
/// surfaces an error to the operator and the chat keeps working - the
/// behavior that replaces the relay's gateway-health short-circuit.
#[tokio::test]
async fn gate_model_failure_surfaces_an_error_and_the_next_input_works() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "fail").await;
    let error = socket
        .recv_until(Duration::from_secs(10), |frame| frame["type"] == "error")
        .await;
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("Model turn failed")),
        "the failed model call surfaces as an error frame naming the boundary: {error}"
    );

    // The pcall'd failure never kills the program: the loop returns to
    // user_input and the next turn is a normal one.
    let fresh = next_wait_token(&mut socket).await;
    answer(&mut socket, &fresh, "recovered").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:recovered",
        "the next input still works after the failure"
    );
    let reply = turn.events.last().expect("the recovery turn completes");
    assert_eq!(reply["event"]["content"], "echo:recovered");
    socket.close().await;
}
