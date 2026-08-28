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
use crate::lua::{LuaProgram, current_tool_bindings};
use crate::model::{CompletionOptions, ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use crate::observe::{NullObserver, Observation, detail};
use crate::tools::{Tool, ToolCatalog, ToolError, ToolErrorKind, ToolId, ToolOutput};
use crate::untrusted::GuardNonce;

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
    "```lua shared\nmodels.default('writer', 'A general model for tests')\n```\n\n";

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
    }
}

fn ensure_model_h1(md: &str) -> String {
    let first_section = md.find("\n\n## ");
    let mut source = md.to_string();
    if source.contains("models.default") || source.contains("models.bind") {
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

/// Builds a client pointed at the given scripted gateway.
fn gateway_client(addr: SocketAddr) -> GatewayClient {
    GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    )
}

/// Options that report nowhere and point the run's client at the given
/// scripted gateway.
fn gatewayed(addr: SocketAddr) -> RunOptions {
    RunOptions {
        execution: EXECUTION,
        observer: Arc::new(NullObserver),
        client: Some(gateway_client(addr)),
        debug: None,
    }
}

/// Options that point at a scripted gateway and record debug events.
fn gatewayed_with_debug(addr: SocketAddr, capture: Arc<dyn DebugCapture>) -> RunOptions {
    RunOptions {
        debug: Some(capture),
        ..gatewayed(addr)
    }
}

/// True when the host exports no gateway configuration, so a test asserting
/// the lazy-client construction error cannot be turned into a real gateway
/// call by a developer's PROMPTFORGE_GATEWAY_* variables.
fn gateway_env_is_unset() -> bool {
    std::env::var_os("PROMPTFORGE_GATEWAY_URL").is_none()
        && std::env::var_os("PROMPTFORGE_GATEWAY_API_KEY").is_none()
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
    let tool_catalog = ToolCatalog::new(tools).expect("fixture tools are unique");
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
        ResolutionContext::new(&picker, &test.models, &tool_catalog),
        store,
        run_config,
    )
    .await
    .map_err(Error::from)
}

/// Runs a fixture offline through the real [`run`](super::run) entry point
/// with a caller-customized [`RunConfig`], returning the typed [`RunError`]
/// so a test can assert on its kind (limits, cancellation).
async fn run_with_config(
    test: &TestPrompt,
    configure: impl FnOnce(RunConfig) -> RunConfig,
) -> std::result::Result<String, RunError> {
    let picker = ToolPicker::build(
        Catalog::new(Vec::new()),
        PickerConfig::default()
            .with_similarity_floor(0.0)
            .and_then(|config| config.with_margin(0.0))
            .expect("test thresholds are in the supported domain"),
    )
    .expect("test picker must build");
    super::run(
        &test.prompt,
        "",
        ResolutionContext::new(&picker, &test.models, &ToolCatalog::default()),
        &StoreRef::memory(),
        configure(RunConfig::new(EXECUTION)),
    )
    .await
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

/// The single configurable mock gateway every execution test uses
/// (EXEC-TESTS-005). It serves a fixed script of chat-completions responses in
/// order, repeating the last entry once the script is exhausted, records every
/// request body it receives, and counts calls.
///
/// The server is OWNED (EXEC-TESTS-003): the guard holds the bound address, a
/// graceful-shutdown sender, and the serving task's `JoinHandle`. Dropping the
/// guard (at test end) signals shutdown and aborts the task, so no detached
/// server survives the test to `.unwrap()`-panic during runtime teardown. The
/// listener is bound inside [`ScriptedGateway::start`], so a bind failure
/// surfaces in the owning test, not in a detached task.
struct ScriptedGateway {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Value>>>,
    calls: Arc<AtomicUsize>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: tokio::task::JoinHandle<()>,
}

/// One scripted reply: either a JSON completion body (HTTP 200) or a
/// status-coded error body, so one harness covers success and backend-failure
/// tests alike.
#[derive(Clone)]
enum GatewayReply {
    Json(Value),
    Status(u16, String),
    DelayedJson(std::time::Duration, Value),
}

#[derive(Clone)]
struct ScriptState {
    responses: Arc<Vec<GatewayReply>>,
    requests: Arc<Mutex<Vec<Value>>>,
    calls: Arc<AtomicUsize>,
}

impl ScriptedGateway {
    /// Starts a gateway serving `responses` in order (repeating the last).
    async fn start(responses: Vec<GatewayReply>) -> ScriptedGateway {
        async fn completions(
            State(state): State<ScriptState>,
            Json(body): Json<Value>,
        ) -> axum::response::Response {
            use axum::response::IntoResponse;
            let n = state.calls.fetch_add(1, Ordering::SeqCst);
            state
                .requests
                .lock()
                .expect("scripted gateway request log must not be poisoned")
                .push(body);
            let index = n.min(state.responses.len() - 1);
            match &state.responses[index] {
                GatewayReply::Json(value) => Json(value.clone()).into_response(),
                GatewayReply::Status(code, body) => (
                    StatusCode::from_u16(*code).expect("valid test status code"),
                    body.clone(),
                )
                    .into_response(),
                GatewayReply::DelayedJson(delay, value) => {
                    tokio::time::sleep(*delay).await;
                    Json(value.clone()).into_response()
                }
            }
        }

        assert!(
            !responses.is_empty(),
            "a scripted gateway needs at least one response"
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let state = ScriptState {
            responses: Arc::new(responses),
            requests: Arc::clone(&requests),
            calls: Arc::clone(&calls),
        };
        let router = Router::new()
            .route("/v1/chat/completions", post(completions))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("scripted gateway must bind a local port");
        let addr = listener
            .local_addr()
            .expect("scripted gateway must report its local address");
        let (shutdown, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            // No `.unwrap()`: the serve outcome is swallowed so a torn-down test
            // runtime can never trigger a detached-task panic (EXEC-TESTS-003).
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        ScriptedGateway {
            addr,
            requests,
            calls,
            shutdown: Some(shutdown),
            server,
        }
    }

    /// The bound local address (`127.0.0.1:<port>`).
    fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The number of completion requests served so far.
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// A snapshot of every recorded request body, in arrival order.
    fn requests(&self) -> Vec<Value> {
        self.requests
            .lock()
            .expect("scripted gateway request log must not be poisoned")
            .clone()
    }

    /// The most recently recorded request body, if any.
    fn last_request(&self) -> Option<Value> {
        self.requests
            .lock()
            .expect("scripted gateway request log must not be poisoned")
            .last()
            .cloned()
    }
}

impl Drop for ScriptedGateway {
    fn drop(&mut self) {
        // Signal graceful shutdown, then abort to guarantee the task ends with
        // the guard rather than outliving the test as a detached server.
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.server.abort();
    }
}

/// A response asking the model to call one tool (OpenAI `tool_calls` shape).
fn resp_tool_call(id: &str, name: &str, arguments: &str) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": arguments }
                }]
            }
        }]
    }))
}

/// A final assistant text reply.
fn resp_text(content: &str) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "message": { "role": "assistant", "content": content }
        }]
    }))
}

/// A delayed final assistant text reply for in-flight cancellation tests.
fn resp_delayed_text(content: &str, delay: std::time::Duration) -> GatewayReply {
    GatewayReply::DelayedJson(
        delay,
        json!({
            "choices": [{
                "message": { "role": "assistant", "content": content }
            }]
        }),
    )
}

/// A final assistant text reply carrying an explicit `finish_reason`.
fn resp_text_finish(content: &str, finish_reason: &str) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "finish_reason": finish_reason,
            "message": { "role": "assistant", "content": content }
        }]
    }))
}

/// A status-coded error response with a raw body (for backend-failure tests).
fn resp_status(code: u16, body: &str) -> GatewayReply {
    GatewayReply::Status(code, body.to_owned())
}

/// The two-round `echo` tool-call-then-text script most loop tests use.
fn echo_then_text_script() -> Vec<GatewayReply> {
    vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ]
}

/// A tool-call under `alias` on the first round, then a final text reply.
fn aliased_tool_script(alias: &str) -> Vec<GatewayReply> {
    vec![
        resp_tool_call("aliased_call", alias, "{\"value\":\"payload\"}"),
        resp_text("aliased final"),
    ]
}

/// Build the tool schemas the loop advertises, mirroring what `run` does.
fn schemas_for(tools: &[Arc<dyn Tool>]) -> Vec<ToolSchema> {
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

/// Build the loop's dispatch map: each fixture tool bound under its wire name,
/// mirroring what `prepare_scoped_tools` produces for an always-scoped bind.
fn dispatch_for(tools: &[Arc<dyn Tool>]) -> BTreeMap<String, DispatchTarget> {
    tools
        .iter()
        .map(|tool| {
            (
                tool.wire_name().to_owned(),
                DispatchTarget::Bound(crate::lua::ToolBinding::for_test(
                    tool.wire_name(),
                    tool.description(),
                    Arc::clone(tool),
                )),
            )
        })
        .collect()
}

/// The test-only port of the deleted production `run_tool_loop` wrapper: a
/// fresh conversation looping until text, with exhaustion surfaced as
/// [`Error::ToolLoopExhausted`]. The loop tests keep their original call
/// shape through this shim over [`run_prose_inference`]; every call reports
/// under [`EXECUTION`] with no debug capture.
#[expect(
    clippy::too_many_arguments,
    reason = "the shim mirrors the deleted wrapper's borrowed loop context"
)]
async fn run_tool_loop(
    client: &GatewayClient,
    schemas: &[ToolSchema],
    dispatch: &BTreeMap<String, DispatchTarget>,
    prose: String,
    max_tool_iterations: usize,
    observer: &dyn Observer,
    section: &str,
    turns: &AtomicU32,
    completion_options: &CompletionOptions,
    nonce: &GuardNonce,
    counts: Option<&ToolCallCounts>,
    global_aliases: Option<&BTreeMap<String, ToolId>>,
    local_dispatch: Option<&LocalDispatch<'_>>,
) -> Result<(String, Option<String>)> {
    let mut conversation = Vec::new();
    let outcome = run_prose_inference(
        client,
        schemas,
        dispatch,
        &mut conversation,
        prose,
        ProseMode::Loop {
            max_tool_iterations,
        },
        EXECUTION,
        observer,
        section,
        turns,
        None,
        completion_options,
        nonce,
        counts,
        global_aliases,
        local_dispatch,
    )
    .await?;
    match outcome.text {
        Some(text) => Ok((text, outcome.finish_reason)),
        None => Err(Error::ToolLoopExhausted),
    }
}

// --- Schema description overrides (ported from the deleted tool_bag.rs) ---
//
// `ToolBag::prepare` wrapped exactly this construction -
// `current_tool_bindings` plus `prepare_scoped_tools` - so the schema-level
// override coverage ports onto the prose path's scope building directly. The
// bag's generation cache is deleted with the bag, so the cache test has no
// behavior left to port; per-block scope rebuilds stay covered by the
// tool-scoping and fanout-arm suites.

/// The catalog text is advertised when no override exists at any layer, and a
/// `tools.add` override reaches the advertised schema.
#[test]
fn tool_description_override_appears_in_model_schema() {
    let echo: Arc<dyn Tool> = Arc::new(EchoTool);
    let bindings = crate::lua::ToolSet::for_test(
        vec![crate::lua::ToolBinding::for_test(
            "echo",
            "echo capability for live matching",
            Arc::clone(&echo),
        )],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::fresh(),
        &bindings,
        &ModelSet::default(),
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("captured bindings must install");
    vm.install_captured_bindings()
        .expect("alias globals must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");

    // tools.add(alias) with no override keeps the bound tool's catalog text.
    let add_default = LuaProgram::compile(
        "tools.add(echo)",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("prologue must compile");
    vm.run_chunk(&add_default, &NullObserver, "Override")
        .expect("tools.add(echo) without override must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(
        schemas[0].description,
        echo.description(),
        "no override anywhere must advertise the bound tool's description"
    );

    // tools.add(alias, override) overrides the model-facing schema.
    let add_override = LuaProgram::compile(
        "tools.add('echo', 'Author override for the model')",
        "prologue-2",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("second prologue must compile");
    vm.run_chunk(&add_override, &NullObserver, "Override")
        .expect("description override at tools.add must succeed");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas[0].description, "Author override for the model");

    vm.teardown(&NullObserver, "Override");
}

/// Precedence at the advertised schema: a `tools.add` override beats the
/// `model_description` recorded by `tools.bind` / `tools.always`, which itself
/// beats the catalog text.
#[test]
fn bind_override_reaches_the_schema_and_add_beats_bind() {
    let bindings = crate::lua::ToolSet::for_test(
        vec![crate::lua::ToolBinding {
            alias: "echo".to_owned(),
            description: "echo capability for live matching".to_owned(),
            id: ToolId::new("tests", "echo").expect("valid id"),
            model_description: Some("bind override".to_owned()),
            tool: Arc::new(EchoTool),
            conflicts: Vec::new(),
        }],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::fresh(),
        &bindings,
        &ModelSet::default(),
        EXECUTION,
        &NullObserver,
        "Precedence",
    )
    .expect("captured bindings must install");
    vm.install_captured_bindings()
        .expect("alias globals must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");

    let add_plain = LuaProgram::compile(
        "tools.add('echo')",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Precedence",
    )
    .expect("prologue must compile");
    vm.run_chunk(&add_plain, &NullObserver, "Precedence")
        .expect("tools.add without override must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(
        schemas[0].description, "bind override",
        "the bind/always override must beat the catalog text"
    );

    let add_override = LuaProgram::compile(
        "tools.add('echo', 'add override')",
        "prologue-2",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Precedence",
    )
    .expect("second prologue must compile");
    vm.run_chunk(&add_override, &NullObserver, "Precedence")
        .expect("tools.add with override must succeed");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(
        schemas[0].description, "add override",
        "the add override must beat the bind/always override"
    );

    vm.teardown(&NullObserver, "Precedence");
}

#[tokio::test]
async fn tool_loop_dispatches_then_returns_text() {
    // The loop is tested against a real client pointed at the mock gateway.
    // `run_tool_loop` takes the client explicitly, so no process-global env
    // is needed (the crate forbids `unsafe`, which `env::set_var` requires).
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let addr = gateway.addr();
    let client = gateway_client(addr);

    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(EchoTool)];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver,
        "Only",
        &turns,
        &options,
        &nonce,
        None,
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
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        // Matches the function name the mock gateway asks for.
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
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

    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let addr = gateway.addr();
    let client = gateway_client(addr);
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(SlowTool)];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();

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
            "ask the model".to_string(),
            DEFAULT_MAX_TOOL_ITERATIONS,
            &NullObserver,
            "Only",
            &turns,
            &options,
            &nonce,
            None,
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

#[tokio::test]
async fn run_with_a_pre_cancelled_handle_fails_as_cancelled() {
    use crate::cancel::CancelHandle;

    // The explicit-cancel wiring of the public entry point: a handle passed
    // through `RunConfig::cancel` is installed around the whole run body, so
    // the section's Lua instruction hook observes it and the run maps the
    // interruption to `RunErrorKind::Cancelled`.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Loop\n\n```lua\nlocal n = 0\nwhile true do n = n + 1 end\n```\n";
    let handle = CancelHandle::new();
    handle.cancel();
    let error = run_with_config(&fixture(md), |config| config.cancel(handle))
        .await
        .expect_err("a pre-cancelled handle must fail the run");
    assert!(
        matches!(error.kind(), RunErrorKind::Cancelled),
        "expected RunErrorKind::Cancelled, got {error:?}"
    );
    assert!(error.is_cancelled());
}

/// Run the loop against `addr` with `tools` in scope, recording observations
/// and the turn count so tests can assert on the accepted or failed turn.
async fn run_tool_loop_recorded(
    addr: SocketAddr,
    tools: &[Arc<dyn Tool>],
) -> (Result<String>, Vec<(String, String)>, u32) {
    let client = gateway_client(addr);
    let recorder = Arc::new(Recorder::default());
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let out = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        recorder.as_ref(),
        "Gather",
        &turns,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .map(|(text, _)| text);
    (out, recorder.events(), turns.load(Ordering::Relaxed))
}

#[tokio::test]
async fn empty_final_text_fails_the_turn() {
    // No prior tool calls: an empty "stop" turn on the first round is a
    // failure, not a clean exit.
    let gateway = ScriptedGateway::start(vec![resp_text_finish("", "stop")]).await;
    let addr = gateway.addr();
    let (out, events, turns) = run_tool_loop_recorded(addr, &[]).await;
    assert!(matches!(out, Err(Error::EmptyModelReply { .. })));
    assert_eq!(turns, 0);
    assert_eq!(
        events,
        vec![("Gather".to_string(), detail::MODEL_TURN_FAILED.to_string(),)]
    );
}

#[tokio::test]
async fn length_finish_reason_reports_model_turn_truncated() {
    let gateway = ScriptedGateway::start(vec![resp_text_finish("partial answer", "length")]).await;
    let addr = gateway.addr();
    let (out, events, turns) = run_tool_loop_recorded(addr, &[]).await;
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
    // `finish_reason: "length"` is never a clean exit, even with empty text.
    let gateway = ScriptedGateway::start(vec![resp_text_finish("", "length")]).await;
    let addr = gateway.addr();
    let (out, events, turns) = run_tool_loop_recorded(addr, &[]).await;
    assert!(matches!(out, Err(Error::EmptyModelReply { .. })));
    assert_eq!(turns, 0);
    assert_eq!(
        events,
        vec![("Gather".to_string(), detail::MODEL_TURN_FAILED.to_string(),)]
    );
}

#[tokio::test]
async fn empty_stop_turn_after_tool_call_is_a_clean_exit() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text_finish("", "stop"),
    ])
    .await;
    let addr = gateway.addr();
    let echo: Arc<dyn Tool> = Arc::new(EchoTool);
    let (out, events, turns) = run_tool_loop_recorded(addr, &[echo]).await;
    assert_eq!(
        out.as_deref().expect("the run must succeed"),
        "",
        "a clean stop-exit yields an empty reply"
    );
    assert_eq!(turns, 2, "the tool-call turn and the accepted empty turn");
    assert_eq!(
        events,
        vec![
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
            (
                "Gather".to_string(),
                detail::TOOL_CALL_SUCCEEDED.to_string(),
            ),
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
        ]
    );
}

#[tokio::test]
async fn empty_stop_turn_without_tool_calls_fails() {
    // Zero prior dispatches: the acceptance conditions cannot hold, so the
    // empty "stop" turn stays an `EmptyModelReply` failure.
    let gateway = ScriptedGateway::start(vec![resp_text_finish("", "stop")]).await;
    let addr = gateway.addr();
    let echo: Arc<dyn Tool> = Arc::new(EchoTool);
    let (out, events, turns) = run_tool_loop_recorded(addr, &[echo]).await;
    assert!(matches!(out, Err(Error::EmptyModelReply { .. })));
    assert_eq!(turns, 0);
    assert_eq!(
        events,
        vec![("Gather".to_string(), detail::MODEL_TURN_FAILED.to_string(),)]
    );
}

#[tokio::test]
async fn empty_turn_without_finish_reason_after_tool_call_fails() {
    // Fail closed: a missing finish reason is not "stop", so the empty turn
    // is an error even after a successful dispatch.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text(""),
    ])
    .await;
    let addr = gateway.addr();
    let echo: Arc<dyn Tool> = Arc::new(EchoTool);
    let (out, events, turns) = run_tool_loop_recorded(addr, &[echo]).await;
    assert!(matches!(out, Err(Error::EmptyModelReply { .. })));
    assert_eq!(turns, 1, "only the tool-call turn completed");
    assert_eq!(
        events,
        vec![
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
            (
                "Gather".to_string(),
                detail::TOOL_CALL_SUCCEEDED.to_string(),
            ),
            ("Gather".to_string(), detail::MODEL_TURN_FAILED.to_string(),),
        ]
    );
}

// --- Guard-wrapping of untrusted tool results in the loop ---

/// The content of the first `tool`-role message in the last recorded body.
///
/// The second request the loop sends carries the dispatched tool's result;
/// this pulls that result string back out so a test can assert on it.
fn last_tool_turn_content(bodies: &[Value]) -> String {
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
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let addr = gateway.addr();
    let client = gateway_client(addr);

    let echo: Arc<dyn Tool> = Arc::new(UntrustedEchoTool);
    let tools: Vec<Arc<dyn Tool>> = vec![echo];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "ask".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver,
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");

    let content = last_tool_turn_content(&gateway.requests());
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

/// Extracts the guard-tag nonce from every `tool`-role turn in the last body.
fn tool_turn_nonces(bodies: &[Value]) -> Vec<String> {
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
async fn untrusted_nonce_is_stable_across_rounds() {
    // One nonce per run: every round's envelope in a single loop carries the
    // same nonce, so identical content wraps byte-identically and KV-cache
    // prefixes stay shared across rounds.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_0", "echo", "{\"value\":\"hi\"}"),
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ])
    .await;
    let addr = gateway.addr();
    let client = gateway_client(addr);

    let echo: Arc<dyn Tool> = Arc::new(UntrustedEchoTool);
    let tools: Vec<Arc<dyn Tool>> = vec![echo];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "ask".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver,
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");

    let nonces = tool_turn_nonces(&gateway.requests());
    assert!(
        nonces.len() >= 2,
        "expected two rounds of guard-wrapped tool output, got: {nonces:?}"
    );
    assert!(
        nonces.windows(2).all(|pair| pair[0] == pair[1]),
        "every round's untrusted wrap in a run must carry the run's nonce: {nonces:?}"
    );
}

#[tokio::test]
async fn untrusted_nonce_differs_across_runs() {
    // The nonce is minted once per run: two runs of the same prompt wrap the
    // same tool output under different nonces, so an envelope's tag stays
    // unguessable from one run to the next.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.bind('echo', 'echo tool')\n\
        tools.always('echo')\n\
        models.default('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\nUse the tool.\n";
    let mut run_nonces = Vec::new();
    for _ in 0..2 {
        let gateway = ScriptedGateway::start(echo_then_text_script()).await;
        let out = run(
            &bound_with_tools(md, Vec::new()),
            "",
            &[Arc::new(UntrustedEchoTool) as Arc<dyn Tool>],
            &StoreRef::memory(),
            gatewayed(gateway.addr()),
        )
        .await
        .unwrap();
        assert_eq!(out, "final answer");
        let nonces = tool_turn_nonces(&gateway.requests());
        assert_eq!(
            nonces.len(),
            1,
            "each run wraps exactly one tool result, got: {nonces:?}"
        );
        run_nonces.push(nonces.into_iter().next().expect("one nonce"));
    }
    assert_ne!(
        run_nonces[0], run_nonces[1],
        "each run must mint its own nonce"
    );
}

#[tokio::test]
async fn trusted_tool_result_is_appended_verbatim_in_the_loop() {
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let addr = gateway.addr();
    let client = gateway_client(addr);

    let echo: Arc<dyn Tool> = Arc::new(EchoTool);
    let tools: Vec<Arc<dyn Tool>> = vec![echo];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "ask".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver,
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");

    let content = last_tool_turn_content(&gateway.requests());
    assert_eq!(
        content, "echoed: hi",
        "a trusted tool's result must be appended verbatim, got: {content}"
    );
    assert!(
        !content.contains("untrusted_input_"),
        "a trusted tool's result must carry no guard tags, got: {content}"
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

mod debug_and_counts;
mod exec_flow;
mod exit_rules;
mod live_infer;
mod local_tools;
mod model_and_reply;
mod observations;
mod scheduler;
mod tool_loop;
mod tool_scoping;
