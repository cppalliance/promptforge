//! Agent-crate tests over an SSE fixture gateway.
//!
//! The gateway serves a fixed script of buffered chat-completion bodies,
//! each converted to the SSE chunk stream the always-streaming model client
//! consumes, and records every request body - so a test can pin both what
//! the driver sent (advertised tools, wire messages, model name) and what
//! the program received back. The conversion mirrors the executor-side
//! `ScriptedGateway` in `promptforge-core`; test fixtures cannot cross the
//! crate boundary, so the agent crate carries its own.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use serde_json::{Value, json};

use promptforge_core_support::cancel::CancelHandle;
use promptforge_core_support::events::ToolCallEvent;
use promptforge_core_support::observe::{Observation, Observer};
use promptforge_model_client::client::{GatewayClient, GatewayEndpoint, SecretString, StreamDelta};
use promptforge_model_client::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use promptforge_store::StoreRef;
use promptforge_tools::{Tool, ToolCatalog, ToolError, ToolId, ToolOutput};

use crate::agent::run_agent_with_client;
use crate::{AgentConfig, AgentError, AgentLimits};

/// The execution id every fixture run reports under.
const EXECUTION: &str = "agent-chat-test";

/// The agent name every fixture run reports as its `section` label.
const AGENT_NAME: &str = "chat-agent";

/// Splits `text` at its char midpoint, so a scripted string streams as two
/// fragments and the client's accumulation is actually exercised.
fn split_for_stream(text: &str) -> (&str, &str) {
    let mid = text.chars().count() / 2;
    let at = text
        .char_indices()
        .nth(mid)
        .map_or(text.len(), |(index, _)| index);
    text.split_at(at)
}

/// Converts one buffered chat-completion body into the SSE event text a
/// streaming backend would emit for it: reasoning deltas, content split
/// across fragments, tool calls as split argument fragments, the
/// finish-reason chunk, a trailing empty-choices summary chunk when the
/// body carries `usage`/`timings`/`metrics`, and the `[DONE]` sentinel.
fn sse_events(body: &Value) -> String {
    let model = body.get("model").cloned();
    let choice = body["choices"].get(0).cloned().unwrap_or_default();
    let message = choice.get("message").cloned().unwrap_or_default();
    let chunk = |delta: Value, finish: Option<&Value>| -> Value {
        let mut chunk_choice = json!({ "index": 0, "delta": delta });
        if let Some(finish) = finish {
            chunk_choice["finish_reason"] = finish.clone();
        }
        let mut event = json!({ "object": "chat.completion.chunk", "choices": [chunk_choice] });
        if let Some(model) = &model {
            event["model"] = model.clone();
        }
        event
    };
    let mut events: Vec<Value> = Vec::new();
    if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) {
        events.push(chunk(json!({ "reasoning_content": reasoning }), None));
    }
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        let (first, second) = split_for_stream(content);
        for part in [first, second] {
            if !part.is_empty() {
                events.push(chunk(json!({ "content": part }), None));
            }
        }
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for (index, call) in calls.iter().enumerate() {
            let arguments = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (first, second) = split_for_stream(arguments);
            let mut opener = json!({
                "index": index,
                "type": "function",
                "function": {
                    "name": call.pointer("/function/name").cloned().unwrap_or(Value::Null),
                    "arguments": first,
                },
            });
            if let Some(id) = call.get("id") {
                opener["id"] = id.clone();
            }
            events.push(chunk(json!({ "tool_calls": [opener] }), None));
            if !second.is_empty() {
                events.push(chunk(
                    json!({ "tool_calls": [{
                        "index": index,
                        "function": { "arguments": second },
                    }] }),
                    None,
                ));
            }
        }
    }
    let finish = choice.get("finish_reason").cloned().unwrap_or(Value::Null);
    events.push(chunk(json!({}), Some(&finish)));
    let mut summary = serde_json::Map::new();
    for key in ["usage", "timings", "metrics"] {
        if let Some(section) = body.get(key).filter(|section| !section.is_null()) {
            summary.insert(key.to_owned(), section.clone());
        }
    }
    if !summary.is_empty() {
        let mut event = json!({ "object": "chat.completion.chunk", "choices": [] });
        if let Some(model) = &model {
            event["model"] = model.clone();
        }
        for (key, value) in summary {
            event[key] = value;
        }
        events.push(event);
    }
    let mut out = String::new();
    for event in &events {
        out.push_str("data: ");
        out.push_str(&event.to_string());
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

#[derive(Clone)]
struct FixtureState {
    bodies: Arc<Vec<Value>>,
    requests: Arc<Mutex<Vec<Value>>>,
    calls: Arc<AtomicUsize>,
}

/// The SSE fixture gateway: serves scripted completion bodies in order
/// (repeating the last), records every request body, and counts calls.
///
/// The server is owned: the guard holds the bound address, a
/// graceful-shutdown sender, and the serving task's handle, so no detached
/// server survives the test.
struct FixtureGateway {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Value>>>,
    calls: Arc<AtomicUsize>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: tokio::task::JoinHandle<()>,
}

impl FixtureGateway {
    /// Starts a gateway serving `bodies` in order (repeating the last).
    async fn start(bodies: Vec<Value>) -> FixtureGateway {
        async fn completions(
            State(state): State<FixtureState>,
            Json(body): Json<Value>,
        ) -> axum::response::Response {
            use axum::response::IntoResponse;
            let n = state.calls.fetch_add(1, Ordering::SeqCst);
            state
                .requests
                .lock()
                .expect("the fixture request log must not be poisoned")
                .push(body);
            let index = n.min(state.bodies.len() - 1);
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                sse_events(&state.bodies[index]),
            )
                .into_response()
        }

        assert!(
            !bodies.is_empty(),
            "a fixture gateway needs at least one scripted body"
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let state = FixtureState {
            bodies: Arc::new(bodies),
            requests: Arc::clone(&requests),
            calls: Arc::clone(&calls),
        };
        let router = Router::new()
            .route("/v1/chat/completions", post(completions))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the fixture gateway must bind a local port");
        let addr = listener
            .local_addr()
            .expect("the fixture gateway must report its local address");
        let (shutdown, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            // The serve outcome is swallowed so a torn-down test runtime can
            // never trigger a detached-task panic.
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        FixtureGateway {
            addr,
            requests,
            calls,
            shutdown: Some(shutdown),
            server,
        }
    }

    /// A snapshot of every recorded request body, in arrival order.
    fn requests(&self) -> Vec<Value> {
        self.requests
            .lock()
            .expect("the fixture request log must not be poisoned")
            .clone()
    }

    /// The number of completion requests served so far.
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Drop for FixtureGateway {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.server.abort();
    }
}

/// A minimal counting tool, registered under its wire name so a chat test
/// can advertise it and prove the driver never executes what a round
/// returns.
struct FixtureTool {
    id: ToolId,
    wire_name: &'static str,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for FixtureTool {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn wire_name(&self) -> &str {
        self.wire_name
    }

    fn description(&self) -> &'static str {
        "A fixture tool that counts its calls."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
        })
    }

    async fn call(&self, _args: Value) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::trusted("fixture output"))
    }
}

/// Builds one fixture tool and the counter that proves whether it ran.
fn fixture_tool(wire_name: &'static str) -> (Arc<dyn Tool>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let tool = FixtureTool {
        id: ToolId::new("fixture", wire_name).expect("the fixture tool id is valid"),
        wire_name,
        calls: Arc::clone(&calls),
    };
    (Arc::new(tool), calls)
}

/// The fixture model catalog: two catalog models so an `opts.model`
/// override is distinguishable from the `models.use` selection.
fn fixture_models() -> ModelCatalog {
    let context = NonZeroU32::new(4096).expect("4096 is non-zero");
    ModelCatalog::new([
        ModelDescriptor::new(
            ModelId::gateway("test-model").expect("the fixture model name is valid"),
            "the default fixture model",
            context,
            ThinkingMode::Never,
        ),
        ModelDescriptor::new(
            ModelId::gateway("other-model").expect("the fixture model name is valid"),
            "the alternate fixture model",
            context,
            ThinkingMode::Never,
        ),
    ])
    .expect("the fixture catalog has unique models")
}

/// One assistant reply recorded by the [`ContentRecorder`].
struct RecordedReply {
    section: String,
    turn: u32,
    text: String,
    finish_reason: Option<String>,
    model: String,
    has_metrics: bool,
}

/// An [`Observer`] that keeps every content event it is handed, so a test
/// can assert each fires exactly once with its model attribution.
#[derive(Default)]
struct ContentRecorder {
    replies: Mutex<Vec<RecordedReply>>,
    batches: Mutex<Vec<(String, Vec<ToolCallEvent>)>>,
    thinking: Mutex<Vec<(String, String)>>,
}

impl Observer for ContentRecorder {
    fn observe(&self, _execution: &str, _section: &str, _event: Observation) {}

    fn on_assistant_reply(
        &self,
        execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&promptforge_core_support::events::CallMetrics>,
    ) {
        assert_eq!(execution, EXECUTION);
        self.replies
            .lock()
            .expect("the reply log must not be poisoned")
            .push(RecordedReply {
                section: section.to_owned(),
                turn,
                text: text.to_owned(),
                finish_reason: finish_reason.map(str::to_owned),
                model: model.to_owned(),
                has_metrics: metrics.is_some(),
            });
    }

    fn on_assistant_tool_calls(
        &self,
        _execution: &str,
        _section: &str,
        _chain_id: u32,
        _depth: u32,
        _turn: u32,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
        self.batches
            .lock()
            .expect("the batch log must not be poisoned")
            .push((model.to_owned(), calls.to_vec()));
    }

    fn on_thinking(
        &self,
        _execution: &str,
        _section: &str,
        _chain_id: u32,
        _depth: u32,
        _turn: u32,
        model: &str,
        text: &str,
    ) {
        self.thinking
            .lock()
            .expect("the thinking log must not be poisoned")
            .push((model.to_owned(), text.to_owned()));
    }
}

/// An [`AgentConfig`] pointed at `observer`, with the fixture run's fixed
/// name and execution id.
fn config_with(observer: Arc<dyn Observer>) -> AgentConfig {
    AgentConfig {
        name: AGENT_NAME.to_owned(),
        execution: EXECUTION.to_owned(),
        observer,
        cancel: CancelHandle::new(),
        event_log: None,
        on_delta: None,
        ui: None,
        limits: AgentLimits::default(),
    }
}

/// A fixture config that discards every observation.
fn config() -> AgentConfig {
    config_with(Arc::new(
        promptforge_core_support::observe::NullObserver::default(),
    ))
}

/// One completed fixture run: the gateway (its recorded requests), the
/// run-scoped store the program wrote its assertions into, and the run's
/// outcome.
struct FixtureRun {
    gateway: FixtureGateway,
    store: StoreRef,
    result: Result<(), AgentError>,
}

impl FixtureRun {
    /// Reads one store file the agent program wrote.
    fn read(&self, path: &str) -> String {
        self.store
            .read(path)
            .unwrap_or_else(|error| panic!("the program wrote {path}: {error}"))
    }
}

/// Runs `source` as an agent program against a fixture gateway scripted
/// with `bodies`, under the fixture model catalog.
async fn run_over_fixture(
    source: &str,
    bodies: Vec<Value>,
    tools: ToolCatalog,
    config: AgentConfig,
) -> FixtureRun {
    let gateway = FixtureGateway::start(bodies).await;
    let endpoint = GatewayEndpoint::new(&format!("http://{}/v1", gateway.addr))
        .expect("the fixture endpoint is a valid URL");
    let key = SecretString::new("fixture-key").expect("the fixture key is non-empty");
    let client = GatewayClient::new(endpoint, key);
    let store = StoreRef::memory();
    let result = run_agent_with_client(
        source,
        &tools,
        &fixture_models(),
        &store,
        config,
        Some(client),
    )
    .await;
    FixtureRun {
        gateway,
        store,
        result,
    }
}

/// An empty tool catalog for runs that advertise nothing.
fn no_tools() -> ToolCatalog {
    ToolCatalog::new(&[]).expect("an empty catalog is valid")
}

/// A scripted body producing a plain text reply from `model`.
fn text_body(model: &str, content: &str, finish: &str) -> Value {
    json!({
        "model": model,
        "choices": [{
            "message": { "role": "assistant", "content": content },
            "finish_reason": finish,
        }],
    })
}

/// A scripted body producing one `echo` tool call with the given finish
/// reason.
fn tool_call_body(finish: &str) -> Value {
    json!({
        "model": "fixture-model",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "echo", "arguments": "{\"value\":\"hi\"}" },
                }],
            },
            "finish_reason": finish,
        }],
    })
}

#[tokio::test]
async fn a_chat_text_reply_round_trips_with_model_and_metrics() {
    let body = json!({
        "model": "fixture-model",
        "choices": [{
            "message": { "role": "assistant", "content": "Hello agent" },
            "finish_reason": "stop",
        }],
        "usage": { "prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10 },
    });
    let run = run_over_fixture(
        r#"
models.use('test-model')
local result = models.chat({ { role = "user", content = "hi" } })
store.write('reply.txt', result.reply)
store.write('model.txt', result.model)
store.write('finish.txt', result.finish_reason)
store.write('tools_nil.txt', tostring(result.tool_calls == nil))
store.write('total.txt', tostring(result.metrics.usage.total_tokens))
store.write('timed.txt', tostring(result.metrics.client.e2e_ms >= 0))
"#,
        vec![body],
        no_tools(),
        config(),
    )
    .await;
    run.result.as_ref().expect("the chat round completes");
    assert_eq!(run.read("reply.txt"), "Hello agent");
    assert_eq!(run.read("model.txt"), "fixture-model");
    assert_eq!(run.read("finish.txt"), "stop");
    assert_eq!(
        run.read("tools_nil.txt"),
        "true",
        "a text round resumes with tool_calls nil, the presence branch"
    );
    assert_eq!(run.read("total.txt"), "10");
    assert_eq!(
        run.read("timed.txt"),
        "true",
        "client timing must reach the program's metrics table"
    );
    // The no-tools default: the request carries no tools field at all.
    let requests = run.gateway.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].get("tools").is_none(),
        "opts.tools defaults to none: no tools field on the wire"
    );
    assert_eq!(requests[0]["model"], json!("test-model"));
}

#[tokio::test]
async fn chat_tool_calls_return_unexecuted() {
    let (echo, echo_calls) = fixture_tool("echo");
    let tools = ToolCatalog::new(&[echo]).expect("the fixture catalog is valid");
    let run = run_over_fixture(
        r#"
models.use('test-model')
local result = models.chat(
    { { role = "user", content = "call the tool" } },
    { tools = { "echo" } }
)
store.write('reply_nil.txt', tostring(result.reply == nil))
store.write(
    'call.txt',
    result.tool_calls[1].id .. ' ' .. result.tool_calls[1].name
        .. ' ' .. result.tool_calls[1].arguments.value
)
"#,
        vec![tool_call_body("tool_calls")],
        tools,
        config(),
    )
    .await;
    run.result.as_ref().expect("the chat round completes");
    assert_eq!(run.read("reply_nil.txt"), "true");
    assert_eq!(run.read("call.txt"), "call_1 echo hi");
    assert_eq!(
        echo_calls.load(Ordering::SeqCst),
        0,
        "a chat round returns tool calls unexecuted; dispatch is the program's decision"
    );
}

#[tokio::test]
async fn stop_with_tool_calls_still_surfaces_the_tool_calls() {
    // llama.cpp and vLLM routinely finish tool-call rounds with "stop":
    // presence, not finish_reason, is the signal the program branches on.
    let (echo, _) = fixture_tool("echo");
    let tools = ToolCatalog::new(&[echo]).expect("the fixture catalog is valid");
    let run = run_over_fixture(
        r#"
models.use('test-model')
local result = models.chat(
    { { role = "user", content = "call the tool" } },
    { tools = { "echo" } }
)
store.write('present.txt', tostring(result.tool_calls ~= nil))
store.write('reply_nil.txt', tostring(result.reply == nil))
store.write('finish.txt', result.finish_reason)
"#,
        vec![tool_call_body("stop")],
        tools,
        config(),
    )
    .await;
    run.result.as_ref().expect("the chat round completes");
    assert_eq!(run.read("present.txt"), "true");
    assert_eq!(run.read("reply_nil.txt"), "true");
    assert_eq!(run.read("finish.txt"), "stop");
}

#[tokio::test]
async fn opts_model_overrides_the_selection() {
    let run = run_over_fixture(
        r#"
models.use('test-model')
models.chat({ { role = "user", content = "hi" } }, { model = "other-model" })
"#,
        vec![text_body("fixture-model", "ok", "stop")],
        no_tools(),
        config(),
    )
    .await;
    run.result.as_ref().expect("the overridden round completes");
    assert_eq!(
        run.gateway.requests()[0]["model"],
        json!("other-model"),
        "opts.model must override the models.use selection on the wire"
    );

    // With no selection at all, opts.model alone carries the round; without
    // either, the call fails at the call site naming both paths.
    let run = run_over_fixture(
        r#"
models.chat({ { role = "user", content = "hi" } }, { model = "other-model" })
local ok, err = pcall(function()
    return models.chat({ { role = "user", content = "hi" } })
end)
store.write('ok.txt', tostring(ok))
store.write('err.txt', err)
"#,
        vec![text_body("fixture-model", "ok", "stop")],
        no_tools(),
        config(),
    )
    .await;
    run.result
        .as_ref()
        .expect("the opts.model-only run completes");
    assert_eq!(run.read("ok.txt"), "false");
    assert!(
        run.read("err.txt").contains("no model is selected"),
        "a selection-free chat without opts.model names the fix"
    );
    assert_eq!(run.gateway.call_count(), 1);
}

#[tokio::test]
async fn opts_tools_control_the_advertised_set_and_default_to_none() {
    let (echo, _) = fixture_tool("echo");
    let (search, _) = fixture_tool("search");
    let tools = ToolCatalog::new(&[echo, search]).expect("the fixture catalog is valid");
    let run = run_over_fixture(
        r#"
models.use('test-model')
models.chat({ { role = "user", content = "one" } })
models.chat({ { role = "user", content = "two" } }, { tools = { "echo" } })
local ok, err = pcall(function()
    return models.chat({ { role = "user", content = "three" } }, { tools = { "ghost" } })
end)
store.write('unknown_ok.txt', tostring(ok))
store.write('unknown_err.txt', err)
"#,
        vec![text_body("fixture-model", "ok", "stop")],
        tools,
        config(),
    )
    .await;
    run.result
        .as_ref()
        .expect("the advertised-set run completes");
    let requests = run.gateway.requests();
    assert_eq!(
        requests.len(),
        2,
        "the unknown-alias round must fail before any request is sent"
    );
    assert!(
        requests[0].get("tools").is_none(),
        "no opts.tools means no tools field: the default is none"
    );
    let advertised: Vec<&str> = requests[1]["tools"]
        .as_array()
        .expect("the second round advertises tools")
        .iter()
        .map(|tool| {
            tool.pointer("/function/name")
                .and_then(Value::as_str)
                .expect("each advertised tool names its function")
        })
        .collect();
    assert_eq!(
        advertised,
        vec!["echo"],
        "exactly the opts.tools aliases are advertised, nothing else from the catalog"
    );
    assert_eq!(run.read("unknown_ok.txt"), "false");
    assert!(
        run.read("unknown_err.txt")
            .contains("is not registered with this agent"),
        "an unknown alias fails the call naming the miss"
    );
}

#[tokio::test]
async fn an_invalid_message_table_fails_at_the_call_site_naming_the_index() {
    let run = run_over_fixture(
        r#"
models.use('test-model')
local ok, err = pcall(function()
    return models.chat({
        { role = "user", content = "ok" },
        { role = "wizard", content = "x" },
    })
end)
store.write('ok.txt', tostring(ok))
store.write('err.txt', err)
"#,
        vec![text_body("fixture-model", "never fetched", "stop")],
        no_tools(),
        config(),
    )
    .await;
    run.result
        .as_ref()
        .expect("the program catches the call error");
    assert_eq!(run.read("ok.txt"), "false");
    assert!(
        run.read("err.txt").contains("messages[2]"),
        "the validation error names the offending index: {}",
        run.read("err.txt")
    );
    assert_eq!(
        run.gateway.call_count(),
        0,
        "an invalid message table must fail before any request is sent"
    );
}

#[tokio::test]
async fn length_and_content_filter_with_tool_calls_fail_the_batch() {
    // A truncated tool-call batch may hold partial JSON arguments; partial
    // arguments must not execute, so the whole call fails as the answer.
    let (echo, echo_calls) = fixture_tool("echo");
    let tools = ToolCatalog::new(&[echo]).expect("the fixture catalog is valid");
    let run = run_over_fixture(
        r#"
models.use('test-model')
local ok1, err1 = pcall(function()
    return models.chat({ { role = "user", content = "a" } }, { tools = { "echo" } })
end)
local ok2, err2 = pcall(function()
    return models.chat({ { role = "user", content = "b" } }, { tools = { "echo" } })
end)
store.write('oks.txt', tostring(ok1) .. ' ' .. tostring(ok2))
store.write('err1.txt', err1)
store.write('err2.txt', err2)
"#,
        vec![tool_call_body("length"), tool_call_body("content_filter")],
        tools,
        config(),
    )
    .await;
    run.result
        .as_ref()
        .expect("the program catches both failures");
    assert_eq!(run.read("oks.txt"), "false false");
    assert!(
        run.read("err1.txt").contains("truncated") && run.read("err1.txt").contains("length"),
        "the length truncation must reach the program: {}",
        run.read("err1.txt")
    );
    assert!(
        run.read("err2.txt").contains("truncated")
            && run.read("err2.txt").contains("content_filter"),
        "the content_filter truncation must reach the program: {}",
        run.read("err2.txt")
    );
    assert_eq!(echo_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_observer_receives_each_content_event_exactly_once() {
    let (echo, _) = fixture_tool("echo");
    let tools = ToolCatalog::new(&[echo]).expect("the fixture catalog is valid");
    let recorder = Arc::new(ContentRecorder::default());
    let thinking_body = json!({
        "model": "fixture-model",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "considered reply",
                "reasoning_content": "let me think",
            },
            "finish_reason": "stop",
        }],
        "usage": { "prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6 },
    });
    let run = run_over_fixture(
        r#"
models.use('test-model')
models.chat({ { role = "user", content = "think first" } })
models.chat({ { role = "user", content = "now call" } }, { tools = { "echo" } })
"#,
        vec![thinking_body, tool_call_body("tool_calls")],
        tools,
        config_with(Arc::clone(&recorder) as Arc<dyn Observer>),
    )
    .await;
    run.result.as_ref().expect("both rounds complete");

    let replies = recorder.replies.lock().expect("the reply log is intact");
    assert_eq!(
        replies.len(),
        1,
        "exactly one reply event for one text round"
    );
    assert_eq!(replies[0].section, AGENT_NAME);
    assert_eq!(replies[0].turn, 1);
    assert_eq!(replies[0].text, "considered reply");
    assert_eq!(replies[0].finish_reason.as_deref(), Some("stop"));
    assert_eq!(replies[0].model, "fixture-model");
    assert!(
        replies[0].has_metrics,
        "the reply event carries its metrics"
    );

    let thinking = recorder
        .thinking
        .lock()
        .expect("the thinking log is intact");
    assert_eq!(
        *thinking,
        vec![("fixture-model".to_owned(), "let me think".to_owned())],
        "thinking is captured exactly once with its model"
    );

    let batches = recorder.batches.lock().expect("the batch log is intact");
    assert_eq!(
        batches.len(),
        1,
        "exactly one tool-calls event for one tool round"
    );
    assert_eq!(batches[0].0, "fixture-model");
    assert_eq!(batches[0].1.len(), 1);
    assert_eq!(batches[0].1[0].id, "call_1");
    assert_eq!(batches[0].1[0].name, "echo");
    assert_eq!(batches[0].1[0].arguments, json!({ "value": "hi" }));
}

#[tokio::test]
async fn deltas_reach_the_on_delta_callback() {
    let deltas: Arc<Mutex<Vec<StreamDelta>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&deltas);
    let mut config = config();
    config.on_delta = Some(Arc::new(move |delta| {
        sink.lock().expect("the delta log is intact").push(delta);
    }));
    let body = json!({
        "model": "fixture-model",
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Hello agent",
                "reasoning_content": "quick thought",
            },
            "finish_reason": "stop",
        }],
    });
    let run = run_over_fixture(
        r#"
models.use('test-model')
local result = models.chat({ { role = "user", content = "hi" } })
store.write('reply.txt', result.reply)
"#,
        vec![body],
        no_tools(),
        config,
    )
    .await;
    run.result.as_ref().expect("the streamed round completes");
    assert_eq!(run.read("reply.txt"), "Hello agent");
    let deltas = deltas.lock().expect("the delta log is intact");
    let text: String = deltas
        .iter()
        .filter_map(|delta| match delta {
            StreamDelta::Text(fragment) => Some(fragment.as_str()),
            _ => None,
        })
        .collect();
    let reasoning: String = deltas
        .iter()
        .filter_map(|delta| match delta {
            StreamDelta::Reasoning(fragment) => Some(fragment.as_str()),
            _ => None,
        })
        .collect();
    let text_fragments = deltas
        .iter()
        .filter(|delta| matches!(delta, StreamDelta::Text(_)))
        .count();
    assert_eq!(text, "Hello agent");
    assert_eq!(reasoning, "quick thought");
    assert!(
        text_fragments >= 2,
        "the fixture splits content, so the callback must see live fragments, got {text_fragments}"
    );
}

#[tokio::test]
async fn system_and_content_parts_messages_reach_the_wire_verbatim() {
    let run = run_over_fixture(
        r#"
models.use('test-model')
models.chat({
    { role = "system", content = "be terse" },
    { role = "user", content = {
        { type = "text", text = "look" },
        { type = "image_url", image_url = { url = "data:image/png;base64,AA" } },
    } },
})
"#,
        vec![text_body("fixture-model", "ok", "stop")],
        no_tools(),
        config(),
    )
    .await;
    run.result.as_ref().expect("the multimodal round completes");
    assert_eq!(
        run.gateway.requests()[0]["messages"],
        json!([
            { "role": "system", "content": "be terse" },
            { "role": "user", "content": [
                { "type": "text", "text": "look" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AA" } },
            ] },
        ]),
        "system role and content parts must reach the wire exactly as validated"
    );
}

#[tokio::test]
async fn a_models_infer_round_completes_through_the_fixture_gateway() {
    // The shared-kernel path: one completed models.infer round, its reply
    // accumulated from the split SSE fragments.
    let run = run_over_fixture(
        r"
models.use('test-model')
store.write('infer.txt', models.infer('hello'))
",
        vec![text_body("fixture-model", "an inferred reply", "stop")],
        no_tools(),
        config(),
    )
    .await;
    run.result.as_ref().expect("the infer round completes");
    assert_eq!(
        run.read("infer.txt"),
        "an inferred reply",
        "the accumulated stream must resume into the program byte-exact"
    );
    assert_eq!(run.gateway.call_count(), 1);
    assert!(
        run.gateway.requests()[0].get("tools").is_none(),
        "models.infer advertises no tools"
    );
}
