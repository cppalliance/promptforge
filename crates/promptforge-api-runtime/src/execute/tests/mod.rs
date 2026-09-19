//! Unit tests for section execution, tool scoping, and the tool-call loop.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::{Value, json};

use super::gateway::{GatewaySource, env_client_with_limits};
use super::scope::prepare_scoped_tools;
use super::support::{advance_turn, now_rfc3339_checked};
use super::*;
use crate::Result;
use crate::capabilities::CapabilityRegistry;
use crate::client::{GatewayClient, GatewayEndpoint, SecretString};
use crate::debug::DebugCapture;
use crate::lua::{LuaProgram, SectionVm, current_tool_bindings};
use crate::model::{ModelDescriptor, ModelId, ModelSet, ThinkingMode};
use crate::observe::{NullObserver, Observation, Observer, detail};
use crate::store::{Access, StoreError, StoreExt, VfsRef};
use crate::tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};
use crate::untrusted::GuardNonce;
use promptforge_api_types::capabilities::{
    Capability, CapabilityError, CapabilityId, Contribution, RunServices,
};
use promptforge_model_client::model::ModelCatalog;

/// A fresh stock handle's access capability, for tests that inject host
/// values into a standalone VM.
fn fresh_access() -> Arc<Access> {
    Arc::new(
        promptforge_vfs::empty()
            .acquire(shared_vfs::Origin::new("execute test fixture"))
            .expect("the stock backend acquires"),
    )
}

const EXECUTION: &str = "execute-test";

/// F10: compile-time proof that the public execution types are thread-safe.
///
/// `RunContext` carries `Arc<dyn Observer>` / `Arc<dyn DebugCapture>` (shared
/// trait objects) and must be `Send + Sync + 'static` to cross the run's task
/// boundaries; the typed error/limit/result surfaces and the environment must
/// be too.
const fn _public_execution_types_are_send_sync_static() {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync_static::<Environment>();
    assert_send_sync_static::<RunContext>();
    assert_send_sync_static::<RunLimits>();
    assert_send_sync_static::<RunResult>();
    assert_send_sync_static::<RunError>();
    assert_send_sync_static::<RunErrorKind>();
}

/// The runtime's default per-section tool-loop cap, mirrored for tests after the
/// `DEFAULT_MAX_TOOL_ITERATIONS` constant was folded into `RunLimits`.
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 24;

/// The `writer` role declaration every model-facing fixture prompt carries:
/// the frontmatter slot, filled by prepare's trivial fill from the
/// context's current model.
const MODEL_ROLE_DECL: &str = "models:\n  writer: {}\n";

/// The H1 block parking the declared role as the prompt-wide default.
const MODEL_DEFAULT_H1: &str = "```lua\nmodels.default('writer')\n```\n\n";

/// Declares the `writer` role in the prompt's frontmatter, unless the
/// frontmatter already declares roles.
fn declare_writer(source: &str) -> String {
    let frontmatter_end = source.find("\n---\n").expect("frontmatter closes");
    let frontmatter = &source[..frontmatter_end];
    if frontmatter.contains("\nmodels:") {
        return source.to_string();
    }
    let mut out = source.to_string();
    out.insert_str(frontmatter_end + 1, MODEL_ROLE_DECL);
    out
}

/// Lua-only prompts never build the gateway client, so these run offline.
fn parse(md: &str) -> Prompt {
    let source = if md.lines().any(|line| line.starts_with("# ")) {
        md.to_string()
    } else {
        md.replacen("---\n\n", "---\n\n# Test prompt\n\n", 1)
    };
    Prompt::parse(&source, EXECUTION, &NullObserver::default()).unwrap()
}

struct TestPrompt {
    prompt: Prompt,
    models: ModelCatalog,
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

/// Declares the `writer` role and parks it as the prompt-wide default, so a
/// model-facing fixture prompt runs its sections under a bound model.
/// Prompts carrying their own `models.default` call (or the legacy
/// `models.bind` of the removal tests) keep their shape and get only the
/// role declaration.
fn ensure_model_h1(md: &str) -> String {
    let source = md.to_string();
    if source.contains("models.default") || source.contains("models.bind") {
        return declare_writer(&source);
    }
    let source = declare_writer(&source);
    // Positions come from the post-declaration text: the declaration
    // insertion shifts every later index.
    let first_section = source.find("\n\n## ");
    let mut source = source;
    if let Some(marker) = source.find("```lua\n")
        && first_section.is_none_or(|section| marker < section)
    {
        source.replace_range(marker..marker + "```lua".len(), "```lua shared");
        if let Some(pos) = source.find("\n\n## ") {
            source.insert_str(pos + 2, MODEL_DEFAULT_H1);
        }
        return source;
    }
    if let Some(pos) = first_section {
        source.insert_str(pos + 2, MODEL_DEFAULT_H1);
        return source;
    }
    source.replacen(
        "---\n\n",
        &format!("---\n\n# Test prompt\n\n{MODEL_DEFAULT_H1}"),
        1,
    )
}

fn bound_for_model(md: &str) -> TestPrompt {
    TestPrompt {
        prompt: parse(&ensure_model_h1(md)),
        models: test_model_catalog(),
    }
}

// PFCORE-EXEC-TESTS-001: the former `resolver` parameter was a fake seam - it was
// accepted and discarded because live tool binding resolved elsewhere. It has
// been removed so the test helper cannot imply a resolution path it does not
// exercise; exact slots fill by identity against the fixture capability's
// contributed tools at prepare.
fn bound_with_tools(md: &str) -> TestPrompt {
    let mut live_source = md.to_owned();
    if let Some(marker) = live_source.find("```lua shared\n")
        && live_source
            .find("\n\n## ")
            .is_none_or(|section| marker < section)
    {
        live_source.replace_range(marker..marker + "```lua shared".len(), "```lua");
    }
    let source = ensure_model_h1(&live_source);
    TestPrompt {
        prompt: parse(&source),
        models: if source.contains("models.") {
            test_model_catalog()
        } else {
            ModelCatalog::empty()
        },
    }
}

/// Owned run inputs a test supplies: the run name, the progress observer,
/// and optional client/capture sinks. Mirrors the old borrowed `RunOptions`
/// with owned `Arc` instrumentation so it can build a [`RunContext`].
struct RunOptions {
    execution: &'static str,
    observer: Arc<dyn Observer>,
    client: Option<GatewayClient>,
    debug: Option<Arc<dyn DebugCapture>>,
}

/// The test stand-in for the old `StoreRef::memory()`: a stock VFS handle
/// (the store mount preinstalled) whose `read`/`write` helpers each go
/// through a fresh, immediately dropped access. A short-lived access per
/// call is what keeps seeding and post-run assertions conflict-free: the
/// claims model attributes every operation to a live identity, so a held
/// seeder access would meet the run's own identities as a false race.
///
/// The handle is reconnectable: [`run`]'s prepare pass builds the run's
/// own router (a fresh store backend per run), so the wrapper points the
/// store at the prepared handle before driving, and post-run assertions
/// read what the run actually wrote.
struct TestStore(Mutex<VfsRef>);

impl TestStore {
    fn new() -> TestStore {
        TestStore(Mutex::new(promptforge_vfs::empty()))
    }

    /// Wraps a caller-built handle - a gated backend, say - in the test
    /// store's seeding and post-run assertion helpers.
    fn from_vfs(vfs: VfsRef) -> TestStore {
        TestStore(Mutex::new(vfs))
    }

    /// The handle the run and the context builders take.
    fn vfs(&self) -> VfsRef {
        self.0
            .lock()
            .expect("the store lock is not poisoned")
            .clone()
    }

    /// Points the store at the run's prepared handle, so post-run
    /// assertions read the store the run actually used.
    fn reconnect(&self, vfs: VfsRef) {
        *self.0.lock().expect("the store lock is not poisoned") = vfs;
    }

    fn read(&self, path: &str) -> std::result::Result<String, StoreError> {
        let vfs = self.vfs();
        let access = vfs
            .acquire(shared_vfs::Origin::new("TestStore::read"))
            .map_err(StoreError::backend)?;
        vfs.store(&access).read(path)
    }

    fn glob(&self, pattern: &str) -> std::result::Result<Vec<String>, StoreError> {
        let vfs = self.vfs();
        let access = vfs
            .acquire(shared_vfs::Origin::new("TestStore::glob"))
            .map_err(StoreError::backend)?;
        vfs.store(&access).glob(pattern)
    }
}

/// Builds a [`RunContext`] from the test-local [`RunOptions`], for the tests
/// that call [`Environment::run`] directly. The context carries the test
/// model as the current selection, so prepare's trivial fill binds every
/// declared role to it.
fn to_context(opts: RunOptions) -> RunContext {
    let mut ctx = RunContext::new(opts.execution)
        .observer(opts.observer)
        .model(test_model_catalog().models()[0].clone());
    if let Some(client) = opts.client {
        ctx = ctx.client(client);
    }
    if let Some(debug) = opts.debug {
        ctx = ctx.debug(debug);
    }
    ctx
}

/// Options that report nowhere and build no client - what a Lua-only,
/// offline test wants.
fn silent() -> RunOptions {
    RunOptions {
        execution: EXECUTION,
        observer: Arc::new(NullObserver::default()),
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
        observer: Arc::new(NullObserver::default()),
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
    run(&fixture(md), "", &[], &TestStore::new(), silent()).await
}

async fn run(
    test: &TestPrompt,
    args: &str,
    tools: &[Arc<dyn Tool>],
    store: &TestStore,
    opts: RunOptions,
) -> Result<String> {
    let mut env = Environment::new();
    if !tools.is_empty() {
        // The fixture capability contributes the test's tools, so the
        // prompt's declared slots fill against them at prepare.
        env = env.registry(tools_registry(tools));
    }
    let mut ctx = RunContext::new(opts.execution).observer(opts.observer);
    // The host pattern: the context carries the current model, and
    // prepare's trivial fill binds every declared role to it.
    if let Some(model) = test.models.models().first() {
        ctx = ctx.model(model.clone());
    }
    if let Some(client) = opts.client {
        ctx = ctx.client(client);
    }
    if let Some(debug) = opts.debug {
        ctx = ctx.debug(debug);
    }
    // The multi-step path: prepare builds the run's own router (a fresh
    // store backend per run), so the test store reconnects to the
    // prepared handle for its post-run assertions to read what the run
    // actually wrote.
    let (ctx, requirements) = env.prepare(&test.prompt, ctx);
    assert!(
        requirements.is_satisfied(),
        "fixture prompts declare no capabilities or model roles: {requirements:?}"
    );
    store.reconnect(ctx.vfs_handle().clone());
    match super::run(&test.prompt, args, ctx).await {
        RunResult::Ok(output) => Ok(output),
        RunResult::Cancelled => Err(Error::Interrupted),
        RunResult::Failure(error) => Err(Error::from(error)),
    }
}

/// The fixture capability: contributes the test's tools under
/// `tests/tools`, so a prompt's frontmatter tool slots fill against them
/// at prepare - the shape production tools arrive in.
struct FixtureCapability {
    id: CapabilityId,
    tools: Vec<Arc<dyn Tool>>,
}

impl Capability for FixtureCapability {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "The fixture tool capability."
    }

    fn create(&self, services: &RunServices) -> std::result::Result<Contribution, CapabilityError> {
        let _ = services;
        Ok(Contribution {
            tools: self.tools.clone(),
        })
    }
}

/// Builds the registry holding one fixture capability contributing `tools`.
fn tools_registry(tools: &[Arc<dyn Tool>]) -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(FixtureCapability {
            id: CapabilityId::from_validated("tests/tools"),
            tools: tools.to_vec(),
        }))
        .expect("the fixture capability registers");
    registry
}

/// Runs a fixture offline through the real [`Environment::run`] entry point
/// with a caller-customized [`RunContext`], returning the typed [`RunError`]
/// so a test can assert on its kind (limits, cancellation).
async fn run_with_context(
    test: &TestPrompt,
    configure: impl FnOnce(RunContext) -> RunContext,
) -> std::result::Result<String, RunError> {
    let env = Environment::new();
    let mut ctx = configure(RunContext::new(EXECUTION)).vfs(TestStore::new().vfs());
    if ctx.model.is_none()
        && let Some(model) = test.models.models().first()
    {
        ctx = ctx.model(model.clone());
    }
    match env.run(&test.prompt, "", ctx).await {
        RunResult::Ok(output) => Ok(output),
        RunResult::Cancelled => Err(RunError::from(Error::Interrupted)),
        RunResult::Failure(error) => Err(error),
    }
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
        &TestStore::new(),
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
        ToolId::parse("tests/tools/echo").expect("valid id")
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
        ToolId::parse("tests/tools/untrusted_echo").expect("valid id")
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

/// A tool returning a JSON object as text, bound structured in scheduler
/// fixtures so a script `tools.call` resumes it as a Lua table.
struct StructuredFixtureTool {
    /// The exact output text; valid JSON for the happy path, garbage for
    /// the invalid-JSON tool-error path.
    body: &'static str,
    /// Whether the output is trusted. An untrusted output is nonce-wrapped
    /// before the structured classification, so even valid JSON fails the
    /// parse - the ordering that restricts structured output to trusted
    /// tools.
    trusted: bool,
}

#[async_trait::async_trait]
impl Tool for StructuredFixtureTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/structured").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "structured"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Return a structured payload."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, _args: Value) -> std::result::Result<ToolOutput, ToolError> {
        Ok(if self.trusted {
            ToolOutput::trusted(self.body)
        } else {
            ToolOutput::untrusted(self.body)
        })
    }
}

/// A tool whose every call fails, standing in for a tool that hits a broken
/// backend, so a test can observe what the loop reports on its way out.
struct FailingTool;

#[async_trait::async_trait]
impl Tool for FailingTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/failing").expect("valid id")
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
            id: ToolId::parse(&format!("tests/tools/{name}")).expect("valid id"),
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
/// request body it receives, and counts calls. Scripts stay in the buffered
/// chat-completion shape; each is converted to the SSE chunk stream the
/// always-streaming client consumes at serve time (see [`sse_events`]).
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

/// Renders a scripted body as the SSE response the streaming client expects.
fn sse_response(body: &Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        sse_events(body),
    )
        .into_response()
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
                GatewayReply::Json(value) => sse_response(value),
                GatewayReply::Status(code, body) => (
                    StatusCode::from_u16(*code).expect("valid test status code"),
                    body.clone(),
                )
                    .into_response(),
                GatewayReply::DelayedJson(delay, value) => {
                    tokio::time::sleep(*delay).await;
                    sse_response(value)
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

/// A response asking the model to call one tool twice in a single turn.
fn resp_two_tool_calls(name: &str, first: (&str, &str), second: (&str, &str)) -> GatewayReply {
    GatewayReply::Json(json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": first.0,
                        "type": "function",
                        "function": { "name": name, "arguments": first.1 }
                    },
                    {
                        "id": second.0,
                        "type": "function",
                        "function": { "name": name, "arguments": second.1 }
                    }
                ]
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
        &Arc::new(Mutex::new(bindings)),
        &Arc::new(Mutex::new(ModelSet::default())),
        EXECUTION,
        &NullObserver::default(),
        "Override",
    )
    .expect("captured bindings must install");
    vm.install_captured_bindings()
        .expect("alias globals must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");

    // tools.add(alias) with no override keeps the bound tool's catalog text.
    let add_default = LuaProgram::compile(
        "tools.add(echo)",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver::default(),
        "Override",
    )
    .expect("prologue must compile");
    vm.run_chunk(&add_default, &NullObserver::default(), "Override")
        .expect("tools.add(echo) without override must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles().expect("the bag snapshots");
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
        &NullObserver::default(),
        "Override",
    )
    .expect("second prologue must compile");
    vm.run_chunk(&add_override, &NullObserver::default(), "Override")
        .expect("description override at tools.add must succeed");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas[0].description, "Author override for the model");

    vm.teardown(&NullObserver::default(), "Override");
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
            id: ToolId::parse("tests/tools/echo").expect("valid id"),
            model_description: Some("bind override".to_owned()),
            tool: Arc::new(EchoTool),
            output_kind: crate::lua::ToolOutputKind::Plain,
        }],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::fresh(),
        &Arc::new(Mutex::new(bindings)),
        &Arc::new(Mutex::new(ModelSet::default())),
        EXECUTION,
        &NullObserver::default(),
        "Precedence",
    )
    .expect("captured bindings must install");
    vm.install_captured_bindings()
        .expect("alias globals must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");

    let add_plain = LuaProgram::compile(
        "tools.add('echo')",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver::default(),
        "Precedence",
    )
    .expect("prologue must compile");
    vm.run_chunk(&add_plain, &NullObserver::default(), "Precedence")
        .expect("tools.add without override must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles().expect("the bag snapshots");
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
        &NullObserver::default(),
        "Precedence",
    )
    .expect("second prologue must compile");
    vm.run_chunk(&add_override, &NullObserver::default(), "Precedence")
        .expect("tools.add with override must succeed");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(
        schemas[0].description, "add override",
        "the add override must beat the bind/always override"
    );

    vm.teardown(&NullObserver::default(), "Precedence");
}

/// A tool whose call blocks far longer than the test's cancel deadline, so the
/// test can prove the tool-call loop honors cancellation mid-call.
struct SlowTool;

#[async_trait::async_trait]
impl Tool for SlowTool {
    fn id(&self) -> ToolId {
        ToolId::parse("test/tools/slow").expect("valid slow tool id")
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

#[tokio::test]
async fn run_with_a_pre_cancelled_handle_fails_as_cancelled() {
    use crate::cancel::CancelHandle;

    // The explicit-cancel wiring of the public entry point: a handle passed
    // through `RunContext::cancel` is installed around the whole run body, so
    // the section's Lua instruction hook observes it and the run maps the
    // interruption to `RunErrorKind::Cancelled`.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Loop\n\n```lua\nlocal n = 0\nwhile true do n = n + 1 end\n```\n";
    let handle = CancelHandle::new();
    handle.cancel();
    let error = run_with_context(&fixture(md), |ctx| ctx.cancel(handle))
        .await
        .expect_err("a pre-cancelled handle must fail the run");
    assert!(
        matches!(error.kind(), RunErrorKind::Cancelled),
        "expected RunErrorKind::Cancelled, got {error:?}"
    );
    assert!(error.is_cancelled());
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
async fn untrusted_nonce_differs_across_runs() {
    // The nonce is minted once per run: two runs of the same prompt wrap the
    // same untrusted tool result under different nonces, so an envelope's tag
    // stays unguessable from one run to the next.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  echo: tests/tools/untrusted_echo\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('echo', { value = 'hi' })\n```\n";
    let mut run_nonces = Vec::new();
    for _ in 0..2 {
        let out = run(
            &bound_with_tools(md),
            "",
            &[Arc::new(UntrustedEchoTool) as Arc<dyn Tool>],
            &TestStore::new(),
            silent(),
        )
        .await
        .unwrap();
        let marker = "<untrusted_input_";
        let start = out.find(marker).expect("the result is guard-wrapped") + marker.len();
        let end = out[start..]
            .find('>')
            .map(|end| start + end)
            .expect("the guard tag closes");
        run_nonces.push(out[start..end].to_string());
    }
    assert_ne!(
        run_nonces[0], run_nonces[1],
        "each run must mint its own nonce"
    );
}

// --- Progress reporting ---

/// The two-section fixture the fall-through test uses: the first section
/// falls through, the second returns from Lua.
const TWO_SECTIONS: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn \"second\"\n```\n";

const STORE_SECTIONS: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## First\n\n```lua\nstore.write('state.txt', 'first')\n```\n\n\
## Second\n\n```lua\nstore.append('state.txt', '\\nsecond')\nreturn \"second\"\n```\n";

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

mod args_surface;
mod chat_arm;
mod chat_scope;
mod debug_and_counts;
mod exec_flow;
mod exit_rules;
mod input;
mod lazy_prose;
mod live_infer;
mod local_tools;
mod model_and_reply;
mod models_loop;
mod observations;
mod scheduler;
mod tool_call_arm;
mod tool_loop;
mod tool_scoping;
mod unified_pipeline;
