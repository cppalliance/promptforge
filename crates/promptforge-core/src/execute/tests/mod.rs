//! Unit tests for section execution, tool scoping, and the tool-call loop.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use promptforge_tool_picker::{
    Catalog, Config as PickerConfig, ToolAnnotations, ToolDescriptor, ToolId as PickerToolId,
    ToolPicker,
};
use serde_json::Value;

use super::*;
use crate::client::{GatewayClient, GatewayEndpoint, SecretString};
use crate::debug::DebugCapture;
use crate::lua::LuaProgram;
use crate::model::{CompletionOptions, ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use crate::observe::{NullObserver, Observation, detail};
use crate::tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput, ToolRegistry};

const EXECUTION: &str = "execute-test";

/// F10: compile-time proof that the public execution types are thread-safe.
///
/// `RunConfig` carries `Arc<dyn Observer>` / `Arc<dyn DebugCapture>` (shared
/// trait objects) and must be `Send + Sync + 'static` to cross the run's task
/// boundaries; the typed error/limit/resolution surfaces must be too.
const fn _public_execution_types_are_send_sync_static() {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync_static::<RunConfig>();
    assert_send_sync_static::<RunLimits>();
    assert_send_sync_static::<RunError>();
    assert_send_sync_static::<RunErrorKind>();
    // Borrowing resolution context: a fixed concrete lifetime still proves the
    // auto traits hold for its owned shape.
    assert_send_sync::<ResolutionContext<'static>>();
}

/// The runtime's default per-section tool-loop cap, mirrored for tests after the
/// `DEFAULT_MAX_TOOL_ITERATIONS` constant was folded into `RunLimits`.
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 24;

const MODEL_ALWAYS_SHARED: &str =
    "```lua shared\nmodels.always('writer', 'A general model for tests')\n```\n\n";

/// Lua-only prompts never build the gateway client, so these run offline.
fn parse(md: &str) -> Prompt {
    let source = if md.lines().any(|line| line.starts_with("# ")) {
        md.to_string()
    } else {
        md.replacen("---\n\n", "---\n\n# Test prompt\n\n", 1)
    };
    Prompt::parse(&source, EXECUTION, &NullObserver).unwrap()
}

struct TestPrompt {
    prompt: Prompt,
    models: ModelCatalog,
    picker_catalog: Option<Catalog>,
}

impl TestPrompt {
    fn prompt(&self) -> &Prompt {
        &self.prompt
    }
}

/// Build the tool-free parsed form consumed by the complete lifecycle path.
fn fixture(md: &str) -> TestPrompt {
    TestPrompt {
        prompt: parse(md),
        models: ModelCatalog::empty(),
        picker_catalog: None,
    }
}

fn test_model_catalog() -> ModelCatalog {
    let context = NonZeroU32::new(131_072).expect("131072 is non-zero");
    ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("claude-sonnet-4-6").expect("the test model alias is valid"),
        "A general model for tests",
        context,
        ThinkingMode::Switchable,
    )])
    .expect("the test catalog has a single unique model")
}

fn test_completion_options() -> CompletionOptions {
    CompletionOptions {
        model: "claude-sonnet-4-6".to_owned(),
        temperature: None,
        max_tokens: None,
        thinking: None,
        tool_dialect: crate::dialects::ToolDialectId::OpenAi,
    }
}

fn ensure_model_h1(md: &str) -> String {
    let first_section = md.find("\n\n## ");
    let mut source = md.to_string();
    if source.contains("models.always") || source.contains("models.need") {
        source = source
            .replace("```lua shared\nmodels.", "```lua\nmodels.")
            .replace("```lua shared\n  models.", "```lua\n  models.");
        return source;
    }
    if let Some(marker) = source.find("```lua\n")
        && first_section.is_none_or(|section| marker < section)
    {
        source.replace_range(marker..marker + "```lua".len(), "```lua shared");
        if let Some(pos) = source.find("\n\n## ") {
            source.insert_str(pos + 2, &MODEL_ALWAYS_SHARED.replace("lua shared", "lua"));
        }
        return source;
    }
    if let Some(pos) = first_section {
        let mut out = source;
        out.insert_str(pos + 2, &MODEL_ALWAYS_SHARED.replace("lua shared", "lua"));
        return out;
    }
    source.replacen(
        "---\n\n",
        &format!(
            "---\n\n# Test prompt\n\n{}",
            MODEL_ALWAYS_SHARED.replace("lua shared", "lua")
        ),
        1,
    )
}

fn bound_for_model(md: &str) -> TestPrompt {
    TestPrompt {
        prompt: parse(&ensure_model_h1(md)),
        models: test_model_catalog(),
        picker_catalog: None,
    }
}

// PFCORE-EXEC-TESTS-001: the former `resolver` parameter was a fake seam - it was
// accepted and discarded because live tool binding actually resolves through the
// `ToolPicker` built from the catalog in `run`. It has been removed so the test
// helper cannot imply a resolution path it does not exercise.
fn bound_with_tools(
    md: &str,
    near_duplicates: Vec<(ToolDescriptor, ToolDescriptor)>,
) -> TestPrompt {
    let mut live_source = md.to_owned();
    if let Some(marker) = live_source.find("```lua shared\n")
        && live_source
            .find("\n\n## ")
            .is_none_or(|section| marker < section)
    {
        live_source.replace_range(marker..marker + "```lua shared".len(), "```lua");
    }
    let source = ensure_model_h1(&live_source);
    let picker_catalog = if near_duplicates.is_empty() {
        None
    } else {
        Some(Catalog::new(
            near_duplicates
                .into_iter()
                .flat_map(|pair| [pair.0, pair.1])
                .collect(),
        ))
    };
    TestPrompt {
        prompt: parse(&source),
        models: if source.contains("models.") {
            test_model_catalog()
        } else {
            ModelCatalog::empty()
        },
        picker_catalog,
    }
}

/// Owned run inputs a test supplies: the execution id, the progress observer,
/// and optional client/capture sinks. Mirrors the old borrowed `RunOptions`
/// with owned `Arc` instrumentation so it can build a [`RunConfig`].
struct RunOptions {
    execution: &'static str,
    observer: Arc<dyn Observer>,
    client: Option<GatewayClient>,
    debug: Option<Arc<dyn DebugCapture>>,
}

/// Builds a [`RunConfig`] from the test-local [`RunOptions`], for the tests that
/// call [`super::run`] directly with a custom picker and model catalog.
fn to_config(opts: RunOptions) -> RunConfig {
    let mut config = RunConfig::new(opts.execution).observer(opts.observer);
    if let Some(client) = opts.client {
        config = config.client(client);
    }
    if let Some(debug) = opts.debug {
        config = config.debug(debug);
    }
    config
}

/// Options that report nowhere and build no client - what a Lua-only,
/// offline test wants.
fn silent() -> RunOptions {
    RunOptions {
        execution: EXECUTION,
        observer: Arc::new(NullObserver),
        client: None,
        debug: None,
    }
}

/// Parse `md` and run it offline with empty `args`, no tools, and a fresh
/// in-memory store created for the run - the ergonomic path for the
/// Lua-only tests that do not care about the store's contents.
async fn run_offline(md: &str) -> Result<String> {
    run(&fixture(md), "", &[], &StoreRef::memory(), silent()).await
}

async fn run(
    test: &TestPrompt,
    args: &str,
    tools: &[Arc<dyn Tool>],
    store: &StoreRef,
    opts: RunOptions,
) -> Result<String> {
    let catalog = test.picker_catalog.clone().unwrap_or_else(|| {
        Catalog::new(
            tools
                .iter()
                .map(|tool| {
                    let id = tool.id();
                    ToolDescriptor::new(
                        PickerToolId::new(id.server(), id.name()),
                        tool.description(),
                        tool.parameters_schema(),
                    )
                })
                .collect(),
        )
    });
    let config = PickerConfig::default()
        .with_similarity_floor(0.0)
        .and_then(|config| config.with_margin(0.0))
        .expect("test thresholds are in the supported domain");
    let picker = ToolPicker::build(catalog, config).expect("test picker must build");
    let mut run_config = RunConfig::new(opts.execution).observer(opts.observer);
    if let Some(client) = opts.client {
        run_config = run_config.client(client);
    }
    if let Some(debug) = opts.debug {
        run_config = run_config.debug(debug);
    }
    super::run(
        &test.prompt,
        args,
        ResolutionContext::new(&picker, &test.models),
        tools,
        store,
        run_config,
    )
    .await
    .map_err(Error::from)
}

/// An [`Observer`] that keeps every observation it is handed, in order, so a test
/// can assert on the whole sequence rather than on a count.
#[derive(Default)]
struct Recorder(Mutex<Vec<(String, String, String)>>);

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .push((execution.to_owned(), section.to_owned(), event.to_string()));
    }
}

impl Recorder {
    /// The full correlated records recorded so far, in order.
    fn records(&self) -> Vec<(String, String, String)> {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .clone()
    }

    /// The observations recorded so far, in order.
    fn events(&self) -> Vec<(String, String)> {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .iter()
            .map(|(_, section, detail)| (section.clone(), detail.clone()))
            .collect()
    }
}

/// Run `md` offline under a fresh recorder and return the result together
/// with every complete correlated record the recorder saw.
async fn run_recorded(md: &str) -> (Result<String>, Vec<(String, String, String)>) {
    let recorder = Arc::new(Recorder::default());
    let result = run(
        &fixture(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: None,
            debug: None,
        },
    )
    .await;
    (result, recorder.records())
}

/// Discard only the execution field when an older ordering regression is
/// intentionally about section and detail rather than correlation.
fn events(records: &[(String, String, String)]) -> Vec<(String, String)> {
    records
        .iter()
        .map(|(_, section, detail)| (section.clone(), detail.clone()))
        .collect()
}

// Wrap/nonce tests live in the untrusted module; these just confirm wiring.

/// A trivial tool that echoes back the `value` argument it is given.
struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "echo").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Echo the value argument back to the caller."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })
    }

    async fn call(&self, args: Value) -> std::result::Result<ToolOutput, ToolError> {
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::trusted(format!("echoed: {value}")))
    }
}

/// Reads a required string argument from a fixture tool's call arguments.
///
/// The fixtures declare their arguments `required` in their JSON schema, so a
/// missing or non-string value is a malformed call, not something to paper over
/// with an empty string. Returning a concrete [`ToolError`] makes a malformed
/// fixture call fail loudly instead of silently succeeding on `""`.
fn require_string_arg<'a>(args: &'a Value, key: &str) -> std::result::Result<&'a str, ToolError> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| {
        ToolError::message(format!("fixture tool requires a string `{key}` argument"))
            .with_kind(ToolErrorKind::InvalidArguments)
    })
}

/// A tool whose output opts in to guard-wrapping, standing in for a tool
/// like `web_fetch` that returns attacker-controllable text.
struct UntrustedEchoTool;

#[async_trait::async_trait]
impl Tool for UntrustedEchoTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "untrusted_echo").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Echo the value argument back as untrusted external data."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })
    }

    async fn call(&self, args: Value) -> std::result::Result<ToolOutput, ToolError> {
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::untrusted(format!("echoed: {value}")))
    }
}

/// A tool whose every call fails, standing in for a tool that hits a broken
/// backend, so a test can observe what the loop reports on its way out.
struct FailingTool;

#[async_trait::async_trait]
impl Tool for FailingTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "failing").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Always fail."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, _args: Value) -> std::result::Result<ToolOutput, ToolError> {
        // Carry an inner cause so the executor's error can be checked for source
        // preservation (item 4): the tool error must not be flattened to a string.
        let cause = std::io::Error::other("upstream socket reset");
        Err(
            ToolError::with_source("the tool's own backend failed", cause)
                .with_kind(ToolErrorKind::Backend),
        )
    }
}

struct ScopedFixtureTool {
    id: ToolId,
    wire_name: &'static str,
    description: &'static str,
    calls: Arc<AtomicUsize>,
}

impl ScopedFixtureTool {
    fn new(name: &str, wire_name: &'static str, description: &'static str) -> Self {
        Self {
            id: ToolId::new("tests", name).expect("valid id"),
            wire_name,
            description,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl Tool for ScopedFixtureTool {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn wire_name(&self) -> &str {
        self.wire_name
    }

    fn description(&self) -> &str {
        self.description
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"]
        })
    }

    async fn call(&self, args: Value) -> std::result::Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let value = require_string_arg(&args, "value")?;
        Ok(ToolOutput::trusted(format!(
            "called {} with {value}",
            self.id.name(),
        )))
    }
}

/// Spawn a mock gateway that returns a tool call on its first request and a
/// final text reply on its second. The call counter is shared so the two
/// responses are distinguishable.
async fn spawn_mock_gateway() -> SocketAddr {
    async fn completions(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            // First round trip: ask to call the echo tool.
            Json(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "echo",
                                "arguments": "{\"value\":\"hi\"}"
                            }
                        }]
                    }
                }]
            }))
        } else {
            // Second round trip: return the final answer.
            Json(json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "final answer" }
                }]
            }))
        }
    }

    let state = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// Progress that reports nowhere, for the loop tests that assert on the
/// reply rather than on the events. The caller owns the turn counter, so
/// the borrow ends with the call.
fn silent_progress<'a>(
    turns: &'a AtomicU32,
    options: &'a CompletionOptions,
) -> SectionProgress<'a> {
    SectionProgress {
        execution: EXECUTION,
        observer: &NullObserver,
        section: "Only",
        turns,
        debug: None,
        completion_options: options,
    }
}

/// Build the tool schemas the loop advertises, mirroring what `run` does.
fn schemas_for(tools: &[&dyn Tool]) -> Vec<ToolSchema> {
    tools
        .iter()
        .map(|t| {
            ToolSchema::new(
                t.wire_name().to_string(),
                t.description().to_string(),
                t.parameters_schema(),
            )
            .expect("fixture tool schema is valid")
        })
        .collect()
}

fn dispatch_for(tools: &[&dyn Tool]) -> BTreeMap<String, ToolId> {
    tools
        .iter()
        .map(|tool| (tool.wire_name().to_owned(), tool.id()))
        .collect()
}

#[tokio::test]
async fn tool_loop_dispatches_then_returns_text() {
    // The loop is tested against a real client pointed at the mock gateway.
    // `run_tool_loop` takes the client explicitly, so no process-global env
    // is needed (the crate forbids `unsafe`, which `env::set_var` requires).
    let addr = spawn_mock_gateway().await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");
    assert_eq!(
        turns.load(Ordering::Relaxed),
        2,
        "one tool-call reply, then the final text"
    );
}

/// A tool whose call blocks far longer than the test's cancel deadline, so the
/// test can prove the tool-call loop honors cancellation mid-call.
struct SlowTool;

#[async_trait::async_trait]
impl Tool for SlowTool {
    fn id(&self) -> ToolId {
        ToolId::new("test", "slow").expect("valid slow tool id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this to &str"
    )]
    fn wire_name(&self) -> &str {
        // Matches the function name the mock gateway asks for.
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this to &str"
    )]
    fn description(&self) -> &str {
        "a deliberately slow tool"
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn call(&self, _args: Value) -> std::result::Result<ToolOutput, ToolError> {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        Ok(ToolOutput::trusted("done"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_during_in_flight_tool_call_returns_promptly() {
    use crate::cancel::{self, CancelHandle};
    use std::time::{Duration, Instant};

    let addr = spawn_mock_gateway().await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );
    let slow = SlowTool;
    let tools: &[&dyn Tool] = &[&slow];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");
    let turns = AtomicU32::new(0);
    let options = test_completion_options();

    let handle = CancelHandle::new();
    let canceller = handle.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });

    let start = Instant::now();
    let result = cancel::scope(
        handle,
        run_tool_loop(
            &client,
            &schemas,
            &dispatch,
            &registry,
            "ask the model".to_string(),
            DEFAULT_MAX_TOOL_ITERATIONS,
            silent_progress(&turns, &options),
            None,
            None,
        ),
    )
    .await;

    assert!(
        start.elapsed() < Duration::from_secs(5),
        "cancel during an in-flight tool call must return promptly, took {:?}",
        start.elapsed()
    );
    assert!(
        matches!(result, Err(crate::Error::Interrupted)),
        "expected Interrupted, got {result:?}"
    );
}

/// A mock gateway that always asks for a tool call, never converging. The
/// returned counter records how many completion requests it served, so a
/// test can assert the loop stopped after exactly its cap of round trips.
async fn spawn_always_tool_call() -> (SocketAddr, Arc<AtomicUsize>) {
    async fn completions(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_x",
                        "type": "function",
                        "function": { "name": "echo", "arguments": "{\"value\":\"x\"}" }
                    }]
                }
            }]
        }))
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, calls)
}

/// Spawn a mock that answers once with fixed `content` and `finish_reason`.
async fn spawn_text_finish_gateway(
    content: &'static str,
    finish_reason: &'static str,
) -> SocketAddr {
    async fn completions(
        State((content, finish_reason)): State<(&'static str, &'static str)>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        Json(json!({
            "choices": [{
                "finish_reason": finish_reason,
                "message": { "role": "assistant", "content": content }
            }]
        }))
    }

    let router = Router::new().route(
        "/v1/chat/completions",
        post(completions).with_state((content, finish_reason)),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

async fn run_tool_loop_recorded(addr: SocketAddr) -> (Result<String>, Vec<(String, String)>, u32) {
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );
    let recorder = Arc::new(Recorder::default());
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let out = run_tool_loop(
        &client,
        &[],
        &BTreeMap::new(),
        &ToolRegistry::new(std::iter::empty()).expect("unique test registry"),
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        SectionProgress {
            execution: EXECUTION,
            observer: recorder.as_ref(),
            section: "Gather",
            turns: &turns,
            debug: None,
            completion_options: &options,
        },
        None,
        None,
    )
    .await
    .map(|(text, _)| text);
    (out, recorder.events(), turns.load(Ordering::Relaxed))
}

#[tokio::test]
async fn empty_final_text_fails_the_turn() {
    let addr = spawn_text_finish_gateway("", "stop").await;
    let (out, events, turns) = run_tool_loop_recorded(addr).await;
    assert!(matches!(out, Err(Error::EmptyModelReply { .. })));
    assert_eq!(turns, 0);
    assert_eq!(
        events,
        vec![("Gather".to_string(), detail::MODEL_TURN_FAILED.to_string(),)]
    );
}

#[tokio::test]
async fn length_finish_reason_reports_model_turn_truncated() {
    let addr = spawn_text_finish_gateway("partial answer", "length").await;
    let (out, events, turns) = run_tool_loop_recorded(addr).await;
    assert_eq!(out.unwrap(), "partial answer");
    assert_eq!(turns, 1);
    assert_eq!(
        events,
        vec![
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
            (
                "Gather".to_string(),
                detail::MODEL_TURN_TRUNCATED.to_string(),
            ),
        ]
    );
}

#[tokio::test]
async fn empty_truncated_final_text_fails_without_truncation_detail() {
    let addr = spawn_text_finish_gateway("", "length").await;
    let (out, events, turns) = run_tool_loop_recorded(addr).await;
    assert!(matches!(out, Err(Error::EmptyModelReply { .. })));
    assert_eq!(turns, 0);
    assert_eq!(
        events,
        vec![("Gather".to_string(), detail::MODEL_TURN_FAILED.to_string(),)]
    );
}

// --- Guard-wrapping of untrusted tool results in the loop ---

/// Spawn a mock gateway that asks for one `echo` tool call, then returns a
/// final text reply, recording every request body it receives.
///
/// The recorded bodies are the observable seam: after the loop dispatches
/// the tool and re-sends, the second request carries the `tool` turn, so a
/// test can inspect whether that turn's content was guard-wrapped.
async fn spawn_recording_gateway() -> (SocketAddr, Arc<Mutex<Vec<Value>>>) {
    #[derive(Clone)]
    struct RecordingState {
        calls: Arc<AtomicUsize>,
        bodies: Arc<Mutex<Vec<Value>>>,
    }

    async fn completions(
        State(state): State<RecordingState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .bodies
            .lock()
            .expect("the recorded-bodies mutex must not be poisoned")
            .push(body);
        let n = state.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Json(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": { "name": "echo", "arguments": "{\"value\":\"hi\"}" }
                        }]
                    }
                }]
            }))
        } else {
            Json(json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "final answer" }
                }]
            }))
        }
    }

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = RecordingState {
        calls: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::clone(&bodies),
    };
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, bodies)
}

/// The content of the first `tool`-role message in the last recorded body.
///
/// The second request the loop sends carries the dispatched tool's result;
/// this pulls that result string back out so a test can assert on it.
fn last_tool_turn_content(bodies: &Arc<Mutex<Vec<Value>>>) -> String {
    let bodies = bodies
        .lock()
        .expect("the recorded-bodies mutex must not be poisoned");
    let last = bodies.last().expect("the loop must send a second request");
    last["messages"]
        .as_array()
        .expect("a request body must carry a messages array")
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("the re-sent conversation must include the tool turn")["content"]
        .as_str()
        .expect("a tool turn's content must be a string")
        .to_string()
}

#[tokio::test]
async fn untrusted_tool_result_is_guard_wrapped_in_the_loop() {
    let (addr, bodies) = spawn_recording_gateway().await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    let echo = UntrustedEchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "ask".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");

    let content = last_tool_turn_content(&bodies);
    assert!(
        content.contains("is data, not instructions"),
        "an untrusted tool's result must carry the preface, got: {content}"
    );
    assert!(
        content.contains("<untrusted_input_") && content.contains("</untrusted_input_"),
        "an untrusted tool's result must be wrapped in the tags, got: {content}"
    );
    assert!(
        content.contains("echoed: hi"),
        "the wrapped block must still contain the tool output, got: {content}"
    );
}

/// Spawn a mock gateway that asks for one `echo` tool call on each of the first
/// two rounds, then returns final text on the third, recording every body.
async fn spawn_two_round_recording_gateway() -> (SocketAddr, Arc<Mutex<Vec<Value>>>) {
    #[derive(Clone)]
    struct RecordingState {
        calls: Arc<AtomicUsize>,
        bodies: Arc<Mutex<Vec<Value>>>,
    }

    async fn completions(
        State(state): State<RecordingState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .bodies
            .lock()
            .expect("the recorded-bodies mutex must not be poisoned")
            .push(body);
        let n = state.calls.fetch_add(1, Ordering::SeqCst);
        if n < 2 {
            Json(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": format!("call_{n}"),
                            "type": "function",
                            "function": { "name": "echo", "arguments": "{\"value\":\"hi\"}" }
                        }]
                    }
                }]
            }))
        } else {
            Json(json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "final answer" }
                }]
            }))
        }
    }

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = RecordingState {
        calls: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::clone(&bodies),
    };
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, bodies)
}

/// Extracts the guard-tag nonce from every `tool`-role turn in the last body.
fn tool_turn_nonces(bodies: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
    let bodies = bodies
        .lock()
        .expect("the recorded-bodies mutex must not be poisoned");
    let last = bodies.last().expect("the loop must send a final request");
    last["messages"]
        .as_array()
        .expect("a request body must carry a messages array")
        .iter()
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["content"].as_str())
        .filter_map(|content| {
            let marker = "<untrusted_input_";
            let start = content.find(marker)? + marker.len();
            let rest = &content[start..];
            let end = rest.find('>')?;
            Some(rest[..end].to_string())
        })
        .collect()
}

#[tokio::test]
async fn untrusted_nonce_is_fresh_per_round() {
    let (addr, bodies) = spawn_two_round_recording_gateway().await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    let echo = UntrustedEchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "ask".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");

    let nonces = tool_turn_nonces(&bodies);
    assert!(
        nonces.len() >= 2,
        "expected two rounds of guard-wrapped tool output, got: {nonces:?}"
    );
    assert_ne!(
        nonces[0], nonces[1],
        "each round's untrusted wrap must use a fresh nonce, never a reused one"
    );
}

#[tokio::test]
async fn trusted_tool_result_is_appended_verbatim_in_the_loop() {
    let (addr, bodies) = spawn_recording_gateway().await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "ask".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");

    let content = last_tool_turn_content(&bodies);
    assert_eq!(
        content, "echoed: hi",
        "a trusted tool's result must be appended verbatim, got: {content}"
    );
    assert!(
        !content.contains("untrusted_input_"),
        "a trusted tool's result must carry no guard tags, got: {content}"
    );
}

/// Spawn a mock that asks for one `echo` via a sole `tool_code` fence, then
/// returns final text, recording every request body.
async fn spawn_content_fence_recording_gateway() -> (SocketAddr, Arc<Mutex<Vec<Value>>>) {
    #[derive(Clone)]
    struct RecordingState {
        calls: Arc<AtomicUsize>,
        bodies: Arc<Mutex<Vec<Value>>>,
    }

    async fn completions(
        State(state): State<RecordingState>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .bodies
            .lock()
            .expect("the recorded-bodies mutex must not be poisoned")
            .push(body);
        let n = state.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Json(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "```tool_code\necho(value=\"hi\")\n```"
                    }
                }]
            }))
        } else {
            Json(json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "final answer" }
                }]
            }))
        }
    }

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = RecordingState {
        calls: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::clone(&bodies),
    };
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, bodies)
}

#[tokio::test]
async fn content_fence_tool_loop_echoes_user_tool_result() {
    let (addr, bodies) = spawn_content_fence_recording_gateway().await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");

    let turns = AtomicU32::new(0);
    let options = CompletionOptions {
        model: "gemma-3-27b-it".to_owned(),
        temperature: None,
        max_tokens: None,
        thinking: None,
        tool_dialect: crate::dialects::ToolDialectId::Gemma3ToolCode,
    };
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "ask".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");

    let bodies = bodies
        .lock()
        .expect("the recorded-bodies mutex must not be poisoned");
    let last = bodies.last().expect("the loop must send a second request");
    let messages = last["messages"]
        .as_array()
        .expect("a request body must carry a messages array");
    assert!(
        messages.iter().all(|m| m["role"] != "tool"),
        "content-fence history must not use role=tool: {messages:?}"
    );
    let assistant = messages
        .iter()
        .rev()
        .find(|m| m["role"] == "assistant")
        .expect("re-sent conversation must include the assistant tool_code turn");
    let assistant_content = assistant["content"]
        .as_str()
        .expect("assistant content must be a string");
    assert!(
        assistant_content.contains("```tool_code") && assistant_content.contains("echo("),
        "assistant turn must re-render the tool_code fence, got: {assistant_content}"
    );
    let user = messages
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .expect("re-sent conversation must include the user TOOL RESULT turn");
    let user_content = user["content"]
        .as_str()
        .expect("user content must be a string");
    assert!(
        user_content.contains("TOOL RESULT echo (call_tool_code_0):")
            && user_content.contains("echoed: hi"),
        "user turn must carry TOOL RESULT with the tool body, got: {user_content}"
    );
}

// --- Progress reporting ---

/// The two-section fixture the fall-through test uses: the first section
/// falls through, the second returns from Lua.
const TWO_SECTIONS: &str = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn \"second\"\n```\n";

const STORE_SECTIONS: &str = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
## First\n\n```lua\nstore.write('state.txt', 'first')\n```\n\n\
## Second\n\n```lua\nstore.append('state.txt', '\\nsecond')\nreturn \"second\"\n```\n";

/// Spawn a mock gateway that always answers with the same final text, so a
/// test can drive a prose section without any tool traffic.
async fn spawn_text_gateway() -> SocketAddr {
    async fn completions(Json(_body): Json<Value>) -> Json<Value> {
        Json(json!({
            "choices": [{
                "message": { "role": "assistant", "content": "hello from the mock" }
            }]
        }))
    }

    let router = Router::new().route("/v1/chat/completions", post(completions));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// Spawn a text gateway that captures the first chat-completions body.
async fn spawn_capturing_text_gateway() -> (SocketAddr, Arc<Mutex<Option<Value>>>) {
    async fn completions(
        State(captured): State<Arc<Mutex<Option<Value>>>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        *captured.lock().expect("capture lock") = Some(body);
        Json(json!({
            "choices": [{
                "message": { "role": "assistant", "content": "hello from the mock" }
            }]
        }))
    }

    let captured = Arc::new(Mutex::new(None));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&captured));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, captured)
}

async fn spawn_aliased_tool_gateway(
    alias: &str,
) -> (SocketAddr, Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>) {
    #[derive(Clone)]
    struct AliasState {
        alias: String,
        requests: Arc<AtomicUsize>,
        bodies: Arc<Mutex<Vec<Value>>>,
    }

    async fn completions(State(state): State<AliasState>, Json(body): Json<Value>) -> Json<Value> {
        state.bodies.lock().unwrap().push(body);
        let request = state.requests.fetch_add(1, Ordering::SeqCst);
        if request == 0 {
            Json(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "aliased_call",
                            "type": "function",
                            "function": {
                                "name": state.alias,
                                "arguments": "{\"value\":\"payload\"}"
                            }
                        }]
                    }
                }]
            }))
        } else {
            Json(json!({
                "choices": [{
                    "message": {"role": "assistant", "content": "aliased final"}
                }]
            }))
        }
    }

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::new(AtomicUsize::new(0));
    let state = AliasState {
        alias: alias.to_owned(),
        requests: Arc::clone(&requests),
        bodies: Arc::clone(&bodies),
    };
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (addr, bodies, requests)
}

fn picker_descriptor(name: &str, description: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        PickerToolId::new("tests", name),
        description,
        json!({"type": "object"}),
    )
    .with_annotations(
        ToolAnnotations::new()
            .with_read_only(true)
            .with_destructive(false)
            .with_idempotent(true),
    )
}

/// The picker's calibrated enriched text for a descriptor.
///
/// The engine's own derivation is crate-private, so this test mirror lets a
/// need equal a tool's embedded text and bind it under any threshold.
fn capability_for(descriptor: &ToolDescriptor) -> String {
    let mut parts: Vec<String> = Vec::new();
    let name = descriptor.name().replace('_', " ");
    if !name.is_empty() {
        parts.push(name);
    }
    if !descriptor.description().is_empty() {
        parts.push(descriptor.description().to_owned());
    }
    let mut params: Vec<&str> = descriptor
        .input_schema()
        .as_object()
        .and_then(|schema| schema.get("properties"))
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().map(String::as_str).collect())
        .unwrap_or_default();
    params.sort_unstable();
    if !params.is_empty() {
        parts.push(format!("parameters: {}", params.join(", ")));
    }
    parts.join(". ")
}

/// Records every [`DebugEvent`] so tests can assert capture wiring.
#[derive(Default)]
struct RecordingCapture(Mutex<Vec<(String, String, u32, crate::debug::DebugEvent)>>);

impl crate::debug::DebugCapture for RecordingCapture {
    fn on_event(
        &self,
        execution: &str,
        section: &str,
        turn_index: u32,
        event: crate::debug::DebugEvent,
    ) {
        self.0
            .lock()
            .expect("the capture mutex must not be poisoned")
            .push((execution.to_owned(), section.to_owned(), turn_index, event));
    }
}

impl RecordingCapture {
    fn events(&self) -> Vec<(String, String, u32, crate::debug::DebugEvent)> {
        self.0
            .lock()
            .expect("the capture mutex must not be poisoned")
            .clone()
    }
}

/// A second fixture tool so the bag can grow from one alias to two.
struct FetchTool;

#[async_trait::async_trait]
impl Tool for FetchTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "fetch").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "fetch"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Fetch a URL."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "url": { "type": "string" } },
            "required": ["url"]
        })
    }

    async fn call(&self, _args: Value) -> std::result::Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted("fetched"))
    }
}

mod debug_and_counts;
mod exec_flow;
mod exit_rules;
mod live_infer;
mod model_and_reply;
mod observations;
mod tool_bag;
mod tool_loop;
mod tool_scoping;
