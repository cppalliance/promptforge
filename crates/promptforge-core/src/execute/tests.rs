//! Unit tests for section execution, tool scoping, and the tool-call loop.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use promptforge_tool_picker::{
    Catalog, Config as PickerConfig, NearDuplicate, ToolAnnotations, ToolDescriptor,
    ToolId as PickerToolId, ToolPicker,
};
use serde_json::Value;

use super::*;
use crate::lua::{
    LuaProgram, SectionVm, ToolCallCounts, bind_shared_declarations, bind_tool_declarations,
};
use crate::model::{
    CompletionOptions, ModelBindings, ModelCatalog, ModelDescriptor, ModelId, ModelInvocation,
    ModelNeedOpts, ResolvedModel, ThinkingMode,
};
use crate::observe::{NullObserver, detail};
use crate::tools::{Tool, ToolId, ToolRegistry};

const EXECUTION: &str = "execute-test";

const MODEL_ALWAYS_SHARED: &str =
    "```lua\nmodels.always('writer', 'A general model for tests')\n```\n\n";

const MODEL_ALWAYS_LINE: &str = "models.always('writer', 'A general model for tests')\n";

/// Lua-only prompts never build the gateway client, so these run offline.
fn parse(md: &str) -> Prompt {
    let source = if md.lines().any(|line| line.starts_with("# ")) {
        md.to_string()
    } else {
        md.replacen("---\n\n", "---\n\n# Test prompt\n\n", 1)
    };
    Prompt::parse(&source, EXECUTION, &NullObserver).unwrap()
}

/// Build the tool-free bound form consumed by the complete lifecycle path.
fn bound(md: &str) -> BoundPrompt {
    BoundPrompt::without_tools(parse(md))
}

fn test_model_catalog() -> ModelCatalog {
    ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("claude-sonnet-4-6"),
        "A general model for tests",
        131_072,
        ThinkingMode::Switchable,
    )])
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

#[expect(clippy::unnecessary_wraps, reason = "ModelResolver requires Result")]
fn fixture_model_resolver(
    _description: &str,
    opts: &ModelNeedOpts,
) -> crate::Result<ResolvedModel> {
    Ok(ResolvedModel {
        id: ModelId::gateway("claude-sonnet-4-6"),
        invocation: ModelInvocation::from(opts),
        tool_dialect: crate::dialects::ToolDialectId::OpenAi,
        context: 200_000,
    })
}

fn ensure_model_shared(md: &str) -> String {
    if md.contains("models.always") || md.contains("models.need") {
        return md.to_string();
    }
    let first_section = md.find("\n\n## ");
    if let Some(marker) = md.find("```lua\n")
        && first_section.is_none_or(|section| marker < section)
    {
        let mut out = md.to_string();
        out.insert_str(marker + 7, MODEL_ALWAYS_LINE);
        return out;
    }
    if let Some(pos) = first_section {
        let mut out = md.to_string();
        out.insert_str(pos + 2, MODEL_ALWAYS_SHARED);
        return out;
    }
    md.replacen(
        "---\n\n",
        &format!("---\n\n# Test prompt\n\n{MODEL_ALWAYS_SHARED}"),
        1,
    )
}

fn bound_for_model(md: &str) -> BoundPrompt {
    let source = ensure_model_shared(md);
    let prompt = parse(&source);
    let no_tools = |_: &str| -> Result<ToolId> {
        Err(Error::Absent {
            capability: "unexpected tool need in model-only fixture".to_owned(),
        })
    };
    let (bindings, models) = if let Some(shared) = &prompt.shared {
        bind_shared_declarations(
            shared,
            &no_tools,
            &fixture_model_resolver,
            EXECUTION,
            &NullObserver,
            &prompt.title,
        )
        .expect("model fixture must bind")
    } else {
        (
            crate::lua::ToolBindings::default(),
            ModelBindings::default(),
        )
    };
    BoundPrompt::with_test_tools(
        prompt,
        bindings,
        BTreeMap::new(),
        BTreeMap::new(),
        Vec::new(),
        models,
    )
}

fn bound_with_tools(
    md: &str,
    resolver: &dyn crate::lua::ToolResolver,
    near_duplicates: Vec<NearDuplicate>,
) -> BoundPrompt {
    let prompt = parse(md);
    let shared = prompt
        .shared
        .as_ref()
        .expect("a tool fixture must declare shared Lua");
    let (bindings, models) = if shared.source().contains("models.") {
        bind_shared_declarations(
            shared,
            resolver,
            &fixture_model_resolver,
            EXECUTION,
            &NullObserver,
            &prompt.title,
        )
        .expect("tool and model fixture must bind")
    } else {
        (
            bind_tool_declarations(shared, resolver, EXECUTION, &NullObserver, &prompt.title)
                .unwrap(),
            ModelBindings::default(),
        )
    };
    let alias_to_id: BTreeMap<String, ToolId> = bindings
        .bindings()
        .iter()
        .map(|binding| (binding.alias().to_owned(), binding.id().clone()))
        .collect();
    BoundPrompt::with_test_tools(
        prompt,
        bindings,
        BTreeMap::new(),
        alias_to_id,
        near_duplicates,
        models,
    )
}

/// Options that report nowhere and build no client - what a Lua-only,
/// offline test wants.
fn silent() -> RunOptions<'static> {
    RunOptions {
        execution: EXECUTION,
        observer: &NullObserver,
        client: None,
        debug: None,
    }
}

/// Parse `md` and run it offline with empty `args`, no tools, and a fresh
/// in-memory store created for the run - the ergonomic path for the
/// Lua-only tests that do not care about the store's contents.
async fn run_offline(md: &str) -> Result<String> {
    run(&bound(md), "", &[], &StoreRef::memory(), silent()).await
}

/// An [`Observer`] that keeps every observation it is handed, in order, so a test
/// can assert on the whole sequence rather than on a count.
#[derive(Default)]
struct Recorder(Mutex<Vec<(String, String, String)>>);

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, detail: &str) {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .push((execution.to_owned(), section.to_owned(), detail.to_owned()));
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
    let recorder = Recorder::default();
    let result = run(
        &bound(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &recorder,
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

#[tokio::test]
async fn falls_through_to_next_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn \"second\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "second");
}

#[tokio::test]
async fn explicit_return_stops_fall_through() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\n```lua\nreturn \"first\"\n```\n\n\
## Second\n\n```lua\nreturn \"unreached\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "first");
}

#[tokio::test]
async fn runs_off_end_to_default_return() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\ndefault_return: \"fell off\"\n---\n\n\
## Only\n\n```lua\nlocal x = 1\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "fell off");
}

#[tokio::test]
async fn generic_result_when_nothing_produced() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nlocal x = 1\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "done");
}

#[tokio::test]
async fn sys_id_increments_per_section() {
    // First section files nothing and falls through; second returns its id.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn tostring(sys.id)\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "2");
}

// --- Version gate at the top of `run` ---

#[tokio::test]
async fn supported_major_one_proceeds() {
    // A `promptforge: 1` prompt clears the gate and runs to completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "ran");
}

#[tokio::test]
async fn unsupported_major_is_refused() {
    // A future major is refused, never silently degraded to major 1.
    let md = "---\nname: t\ndescription: d\npromptforge: 2\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("an unsupported major must be refused");
    assert!(matches!(err, Error::UnsupportedVersion(2)));
}

#[tokio::test]
async fn missing_version_is_not_a_promptforge_prompt() {
    // No `promptforge:` key: not our prompt, so `run` declines with a Parse
    // error rather than executing it.
    let md = "---\nname: t\ndescription: d\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("a prompt with no promptforge version must be declined");
    match err {
        Error::Parse(msg) => assert!(
            msg.contains("not a promptforge prompt"),
            "the Parse message must name the missing version, got: {msg}"
        ),
        other => panic!("expected Parse, got {other:?}"),
    }
}

// --- Cross-section store persistence ---

#[tokio::test]
async fn store_persists_across_sections() {
    // One store is created for the run and threaded to every section. The
    // first section's Lua writes a file; the second, in a fresh context,
    // reads it back - proving the store outlives the context-clearing
    // transition. The read lands in `var`, so it round-trips the value.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Writer\n\n```lua\nstore.write('note.txt', 'carried across')\n```\n\n\
## Reader\n\n```lua\nvar.seen = store.read_lines('note.txt')\nreturn var.seen\n```\n";
    let store = StoreRef::memory();
    let out = run(&bound(md), "", &[], &store, silent()).await.unwrap();
    assert_eq!(
        out, "1| carried across",
        "the second section must read what the first wrote"
    );
    // The very same handle still holds the file after the run, confirming
    // both sections shared one store rather than each getting a fresh one.
    assert_eq!(
        store.read_lines("note.txt").expect("read_lines"),
        "1| carried across",
        "the run's store must retain the written file"
    );
}

// --- Tool-call loop test (exercises the model round trip via a mock) ---

/// A trivial tool that echoes back the `value` argument it is given.
struct EchoTool;

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "echo")
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

    async fn call(&self, args: Value) -> Result<String> {
        let value = args.get("value").and_then(Value::as_str).unwrap_or("");
        Ok(format!("echoed: {value}"))
    }
}

/// A tool whose output opts in to guard-wrapping, standing in for a tool
/// like `web_fetch` that returns attacker-controllable text.
struct UntrustedEchoTool;

#[async_trait::async_trait]
impl Tool for UntrustedEchoTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "untrusted_echo")
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

    async fn call(&self, args: Value) -> Result<String> {
        let value = args.get("value").and_then(Value::as_str).unwrap_or("");
        Ok(format!("echoed: {value}"))
    }

    fn untrusted_output(&self) -> bool {
        true
    }
}

/// A tool whose every call fails, standing in for a tool that hits a broken
/// backend, so a test can observe what the loop reports on its way out.
struct FailingTool;

#[async_trait::async_trait]
impl Tool for FailingTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "failing")
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

    async fn call(&self, _args: Value) -> Result<String> {
        Err(Error::Backend {
            status: 500,
            body: "the tool's own backend failed".to_string(),
        })
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
            id: ToolId::new("tests", name),
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

    async fn call(&self, args: Value) -> Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(format!(
            "called {} with {}",
            self.id.name(),
            args["value"].as_str().unwrap_or_default()
        ))
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
        .map(|t| ToolSchema {
            name: t.wire_name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters_schema(),
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
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied());

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

#[tokio::test]
async fn tool_loop_gives_up_after_exactly_the_configured_cap() {
    // A small explicit cap: the loop must make exactly that many round
    // trips against a never-converging model, then exhaust.
    let cap = 3;
    let (addr, calls) = spawn_always_tool_call().await;
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied());

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "loop forever".to_string(),
        cap,
        silent_progress(&turns, &options),
        None,
        None,
    )
    .await
    .expect_err("a never-converging model should exhaust the loop");
    assert!(matches!(err, Error::ToolLoopExhausted));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        cap,
        "the loop must make exactly `cap` round trips before giving up"
    );
}

#[tokio::test]
async fn tool_loop_uses_the_default_cap_when_unspecified() {
    // Threading `DEFAULT_MAX_TOOL_ITERATIONS` (what `run` passes when a
    // prompt declares no budget) makes exactly that many round trips.
    let (addr, calls) = spawn_always_tool_call().await;
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied());

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "loop forever".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
    )
    .await
    .expect_err("a never-converging model should exhaust the loop");
    assert!(matches!(err, Error::ToolLoopExhausted));
    assert_eq!(calls.load(Ordering::SeqCst), DEFAULT_MAX_TOOL_ITERATIONS);
    assert_eq!(DEFAULT_MAX_TOOL_ITERATIONS, 24);
}

#[test]
fn run_resolves_cap_from_frontmatter_else_default() {
    // Mirrors the resolution in `run`: a declared budget wins, an absent
    // one falls back to the raised default.
    let declared =
        "---\nname: t\ndescription: d\nmax_tool_iterations: 5\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(declared, EXECUTION, &NullObserver).unwrap();
    assert_eq!(
        p.frontmatter
            .max_tool_iterations
            .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS),
        5
    );

    let absent = "---\nname: t\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(absent, EXECUTION, &NullObserver).unwrap();
    assert_eq!(
        p.frontmatter
            .max_tool_iterations
            .unwrap_or(DEFAULT_MAX_TOOL_ITERATIONS),
        DEFAULT_MAX_TOOL_ITERATIONS
    );
}

#[tokio::test]
async fn tool_loop_errors_on_unknown_tool() {
    // The model asks for "echo" but no tools are provided to the loop.
    let (addr, _calls) = spawn_always_tool_call().await;
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    // Advertise schemas so the request carries tools, but pass no dispatch
    // targets, so the returned call resolves to no tool.
    let echo = EchoTool;
    let schemas = schemas_for(&[&echo]);
    let registry = ToolRegistry::new(std::iter::empty());

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let err = run_tool_loop(
        &client,
        &schemas,
        &BTreeMap::new(),
        &registry,
        "call unknown".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
    )
    .await
    .expect_err("an unprovided tool should be rejected");
    match err {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "echo");
            assert!(!global_exists);
            assert!(in_scope.is_empty());
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failing_tool_is_reported_before_the_error_propagates() {
    // The dispatch is split from the `?` precisely so a tool that fails is
    // still reported: the recorder must see `ToolCalled { ok: false }` and
    // the tool's own error must still end the loop.
    let (addr, _calls) = spawn_always_tool_call().await;
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    let failing = FailingTool;
    let tools: &[&dyn Tool] = &[&failing];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied());

    let recorder = Recorder::default();
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        SectionProgress {
            execution: EXECUTION,
            observer: &recorder,
            section: "Gather",
            turns: &turns,
            debug: None,
            completion_options: &options,
        },
        None,
        None,
    )
    .await
    .expect_err("a tool whose call fails must fail the loop");
    match err {
        Error::Backend { status, .. } => assert_eq!(status, 500, "the tool's own error propagates"),
        other => panic!("expected the tool's own error, got {other:?}"),
    }

    assert_eq!(
        recorder.events(),
        vec![
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
            ("Gather".to_string(), detail::TOOL_CALL_FAILED.to_string(),),
        ],
        "the failed dispatch must be reported before the error propagates"
    );
    assert!(
        recorder
            .records()
            .iter()
            .all(|(execution, _, _)| execution == EXECUTION)
    );
}

#[tokio::test]
async fn a_failing_model_turn_is_reported_before_the_error_propagates() {
    async fn fail() -> (StatusCode, &'static str) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "private backend response",
        )
    }

    let router = Router::new().route("/v1/chat/completions", post(fail));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let client = GatewayClient::new(&format!("http://{addr}/v1"), "secret token");
    let recorder = Recorder::default();
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let error = run_tool_loop(
        &client,
        &[],
        &BTreeMap::new(),
        &ToolRegistry::new(std::iter::empty()),
        "private model input".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        SectionProgress {
            execution: EXECUTION,
            observer: &recorder,
            section: "Gather",
            turns: &turns,
            debug: None,
            completion_options: &options,
        },
        None,
        None,
    )
    .await
    .expect_err("the backend failure must propagate");

    assert!(matches!(error, Error::Backend { status: 500, .. }));
    assert_eq!(
        recorder.events(),
        vec![("Gather".to_string(), detail::MODEL_TURN_FAILED.to_string(),)]
    );
    let trace = format!("{:?}", recorder.events());
    for payload in [
        "private backend response",
        "private model input",
        "secret token",
    ] {
        assert!(!trace.contains(payload), "observation leaked {payload:?}");
    }
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
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");
    let recorder = Recorder::default();
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let out = run_tool_loop(
        &client,
        &[],
        &BTreeMap::new(),
        &ToolRegistry::new(std::iter::empty()),
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        SectionProgress {
            execution: EXECUTION,
            observer: &recorder,
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
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    let echo = UntrustedEchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied());

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

#[tokio::test]
async fn trusted_tool_result_is_appended_verbatim_in_the_loop() {
    let (addr, bodies) = spawn_recording_gateway().await;
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied());

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
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied());

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

#[tokio::test]
async fn a_two_section_run_reports_the_exact_observation_sequence() {
    let (result, records) = run_recorded(TWO_SECTIONS).await;
    assert_eq!(result.unwrap(), "second");

    assert_eq!(
        events(&records),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            ("First".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "First".to_string(),
                detail::LUA_PROLOGUE_STARTED.to_string()
            ),
            (
                "First".to_string(),
                detail::LUA_PROLOGUE_SUCCEEDED.to_string(),
            ),
            ("First".to_string(), detail::TOOL_SCOPE_CLOSING.to_string()),
            ("First".to_string(), detail::TOOL_SCOPE_CLOSED.to_string()),
            ("First".to_string(), detail::MODEL_SCOPE_CLOSING.to_string()),
            ("First".to_string(), detail::MODEL_SCOPE_CLOSED.to_string()),
            (
                "First".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string()
            ),
            (
                "First".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("First".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Second".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "Second".to_string(),
                detail::LUA_PROLOGUE_STARTED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_PROLOGUE_SUCCEEDED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Second".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[tokio::test]
async fn recording_and_null_observers_produce_the_same_result_and_store_state() {
    let prompt = bound(STORE_SECTIONS);
    let recorded_store = StoreRef::memory();
    let sink = Recorder::default();
    let observed_result = run(
        &prompt,
        "",
        &[],
        &recorded_store,
        RunOptions {
            execution: EXECUTION,
            observer: &sink,
            client: None,
            debug: None,
        },
    )
    .await;
    let null_store = StoreRef::memory();
    let null_result = run(&prompt, "", &[], &null_store, silent()).await;

    assert_eq!(observed_result.unwrap(), null_result.unwrap());
    assert_eq!(
        recorded_store.glob("**").unwrap(),
        null_store.glob("**").unwrap(),
        "observer choice must not change store side effects"
    );
    assert_eq!(
        recorded_store.read_lines("state.txt").unwrap(),
        null_store.read_lines("state.txt").unwrap(),
        "observer choice must not change stored contents"
    );

    let failing = bound(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
         ## Only\n\n```lua\nerror('expected failure')\n```\n",
    );
    let sink = Recorder::default();
    let observed_error = run(
        &failing,
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &sink,
            client: None,
            debug: None,
        },
    )
    .await
    .expect_err("the prologue fails");
    let null_error = run(&failing, "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("the prologue fails");
    assert_eq!(
        observed_error.to_string(),
        null_error.to_string(),
        "observer choice must not change errors"
    );
}

#[tokio::test]
async fn a_run_refused_by_the_version_gate_reports_nothing() {
    // The gate is not a run that failed; it is a run that never started, so
    // there is no RunStarted to pair a RunFinished with.
    let md = "---\nname: t\ndescription: d\npromptforge: 2\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let (result, records) = run_recorded(md).await;
    assert!(result.is_err());
    assert!(
        records.is_empty(),
        "the gate must report nothing: {records:?}"
    );
}

#[tokio::test]
async fn a_failing_run_still_reports_run_finished() {
    // The prologue fails, so the walk tears down its VM and the final
    // observation must report the run failure.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nerror('expected failure')\n```\n";
    let (result, records) = run_recorded(md).await;
    assert!(matches!(result, Err(Error::Lua(_))));

    assert_eq!(
        events(&records),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            ("Only".to_string(), detail::SECTION_STARTED.to_string()),
            ("Only".to_string(), detail::LUA_PROLOGUE_STARTED.to_string()),
            ("Only".to_string(), detail::LUA_PROLOGUE_FAILED.to_string()),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Test prompt".to_string(), detail::RUN_FAILED.to_string()),
        ],
        "a section that errors reports no SectionFinished"
    );
}

#[tokio::test]
async fn one_execution_id_spans_parse_bind_and_the_complete_runtime_lifecycle() {
    let (addr, _, _) = spawn_aliased_tool_gateway("echo").await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "echo",
        "canonical_echo",
        "Echo a test value.",
    ));
    let descriptor = ToolDescriptor::new(
        PickerToolId::new("tests", "echo"),
        tool.description(),
        tool.parameters_schema(),
    );
    let capability =
        serde_json::to_string(&descriptor.enriched_text()).expect("serialize fixture capability");
    let source = format!(
        "---\nname: lifecycle\ndescription: Correlated lifecycle fixture\npromptforge: 1\n---\n\n\
         # Lifecycle\n\n```lua\n\
         tools.need('echo', {capability})\n\
         tools.always('echo')\n\
         models.always('writer', 'A general model for tests')\n```\n\n\
         ## Gather\n\n```lua\nstore.write('state.txt', 'before')\n```\n\n\
         Use the echo tool.\n\n\
         ```lua\nstore.append('state.txt', '\\nafter')\nreturn reply\n```\n"
    );
    let recorder = Recorder::default();
    let prompt =
        Prompt::parse(&source, EXECUTION, &recorder).expect("the lifecycle fixture must parse");
    let picker = ToolPicker::build(Catalog::new(vec![descriptor]), PickerConfig::default())
        .expect("the lifecycle picker must build");
    let tools: [Arc<dyn Tool>; 1] = [Arc::clone(&tool) as Arc<dyn Tool>];
    let registry = ToolRegistry::new(tools.iter().map(AsRef::as_ref));
    let prompt = crate::bind::bind_prompt(
        prompt,
        &picker,
        &registry,
        &test_model_catalog(),
        EXECUTION,
        &recorder,
    )
    .expect("the lifecycle fixture must bind");
    let store = StoreRef::memory();

    let result = run(
        &prompt,
        "",
        &tools,
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: &recorder,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect("the lifecycle fixture must run");

    assert_eq!(result, "aliased final");
    assert_eq!(
        store.read_lines("state.txt").unwrap(),
        "1| before\n2| after"
    );
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let records = recorder.records();
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .all(|(execution, _, _)| execution == EXECUTION),
        "every lifecycle record must retain {EXECUTION}: {records:#?}"
    );
    let details = records
        .iter()
        .map(|(_, _, detail)| detail.as_str())
        .collect::<Vec<_>>();
    for expected in [
        detail::PARSE_STARTED,
        detail::TOOL_BINDING_STARTED,
        detail::RUN_STARTED,
        detail::SECTION_STARTED,
        detail::LUA_PROLOGUE_STARTED,
        detail::STORE_WRITE_SUCCEEDED,
        detail::MODEL_TURN_COMPLETED,
        detail::TOOL_CALL_SUCCEEDED,
        detail::LUA_EPILOG_STARTED,
        detail::STORE_APPEND_SUCCEEDED,
        detail::RUN_SUCCEEDED,
    ] {
        assert!(
            details.contains(&expected),
            "the complete lifecycle must include {expected:?}: {records:#?}"
        );
    }
}

#[tokio::test]
async fn the_tool_loop_reports_each_turn_and_each_tool_call() {
    let addr = spawn_mock_gateway().await;
    let client = GatewayClient::new(&format!("http://{addr}/v1"), "test");

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied());

    let recorder = Recorder::default();
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        SectionProgress {
            execution: EXECUTION,
            observer: &recorder,
            section: "Gather",
            turns: &turns,
            debug: None,
            completion_options: &options,
        },
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out, "final answer");

    assert_eq!(
        recorder.events(),
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

#[tokio::test]
async fn models_use_forwards_binding_completion_options_to_the_gateway() {
    // models.use -> completion_options -> GatewayClient::complete must carry
    // the binding's model and sampling fields on the chat body.
    let (addr, captured) = spawn_capturing_text_gateway().await;
    let catalog = ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("analyst"),
        "A careful analysis model",
        131_072,
        ThinkingMode::Switchable,
    )]);
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# T\n\n\
```lua\n\
models.need('analyst', 'careful analysis', { temperature = 0.25, max_tokens = 64, thinking = false })\n\
```\n\n\
## Only\n\n\
```lua\nmodels.use('analyst')\n```\n\n\
Ask the model.\n";
    let prompt = Prompt::parse(md, EXECUTION, &NullObserver).expect("fixture must parse");
    let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
        .expect("empty tool picker must build");
    let registry = ToolRegistry::new(std::iter::empty());
    let bound = crate::bind::bind_prompt(
        prompt,
        &picker,
        &registry,
        &catalog,
        EXECUTION,
        &NullObserver,
    )
    .expect("fixture must bind");

    let out = run(
        &bound,
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");

    let body = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("complete must reach the gateway");
    assert_eq!(body["model"], "analyst");
    assert_eq!(body["temperature"], 0.25);
    assert_eq!(body["max_tokens"], 64);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
}

#[tokio::test]
async fn an_explicit_client_is_used_instead_of_the_environment() {
    // `client: Some(..)` is what a caller configured from a file passes;
    // nothing here reads `PROMPTFORGE_*`, and the run still reaches a
    // gateway and reports its model turn.
    let addr = spawn_text_gateway().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\nSay something.\n";
    let recorder = Recorder::default();
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &recorder,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");

    assert_eq!(
        recorder.events(),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            ("Only".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_SHARED_LOAD_STARTED.to_string()
            ),
            ("Only".to_string(), detail::TOOL_REPLAY_STARTED.to_string()),
            ("Only".to_string(), detail::MODEL_REPLAY_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::TOOL_REPLAY_SUCCEEDED.to_string()
            ),
            (
                "Only".to_string(),
                detail::MODEL_REPLAY_SUCCEEDED.to_string()
            ),
            (
                "Only".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string()
            ),
            ("Only".to_string(), detail::TOOL_SCOPE_CLOSING.to_string()),
            ("Only".to_string(), detail::TOOL_SCOPE_CLOSED.to_string()),
            ("Only".to_string(), detail::MODEL_SCOPE_CLOSING.to_string()),
            ("Only".to_string(), detail::MODEL_SCOPE_CLOSED.to_string()),
            (
                "Only".to_string(),
                detail::TOOL_SCOPE_VALIDATION_STARTED.to_string(),
            ),
            (
                "Only".to_string(),
                detail::TOOL_SCOPE_VALIDATION_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::MODEL_TURN_COMPLETED.to_string(),),
            (
                "Only".to_string(),
                detail::LUA_REPLY_BINDING_STARTED.to_string(),
            ),
            (
                "Only".to_string(),
                detail::LUA_REPLY_BINDING_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[tokio::test]
async fn epilog_runs_after_reply_and_can_return() {
    let addr = spawn_text_gateway().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\nSay something.\n\n```lua\nstore.write('epilog-ran.txt', 'yes')\nreturn 'epilog result'\n```\n";
    let prompt = bound_for_model(md);
    assert!(prompt.prompt().entry().prologue().is_none());
    assert!(prompt.prompt().entry().epilog().is_some());

    let recorder = Recorder::default();
    let store = StoreRef::memory();
    let out = run(
        &prompt,
        "",
        &[],
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: &recorder,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "epilog result");
    assert_eq!(store.read_lines("epilog-ran.txt").unwrap(), "1| yes");
    assert_eq!(
        recorder.events(),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            ("Only".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_SHARED_LOAD_STARTED.to_string()
            ),
            ("Only".to_string(), detail::TOOL_REPLAY_STARTED.to_string()),
            ("Only".to_string(), detail::MODEL_REPLAY_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::TOOL_REPLAY_SUCCEEDED.to_string()
            ),
            (
                "Only".to_string(),
                detail::MODEL_REPLAY_SUCCEEDED.to_string()
            ),
            (
                "Only".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string()
            ),
            ("Only".to_string(), detail::TOOL_SCOPE_CLOSING.to_string()),
            ("Only".to_string(), detail::TOOL_SCOPE_CLOSED.to_string()),
            ("Only".to_string(), detail::MODEL_SCOPE_CLOSING.to_string()),
            ("Only".to_string(), detail::MODEL_SCOPE_CLOSED.to_string()),
            (
                "Only".to_string(),
                detail::TOOL_SCOPE_VALIDATION_STARTED.to_string(),
            ),
            (
                "Only".to_string(),
                detail::TOOL_SCOPE_VALIDATION_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::MODEL_TURN_COMPLETED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_REPLY_BINDING_STARTED.to_string(),
            ),
            (
                "Only".to_string(),
                detail::LUA_REPLY_BINDING_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::LUA_EPILOG_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::STORE_WRITE_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::LUA_EPILOG_SUCCEEDED.to_string(),),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[tokio::test]
async fn add_in_an_unbound_prompt_without_declarations_fails_the_run_loudly() {
    // Parsed (unbound) input with no shared library: the run goes through the
    // same validated VM with empty frozen bindings, so the alias is rejected.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\ntools.add('web_search')\n```\n\nThis prose must not reach a model.\n";
    let prompt = parse(md);
    let error = run(&prompt, "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("an undeclared alias must fail the run");
    assert!(
        error.to_string().contains("not declared by tools.need"),
        "the error must report the missing declaration: {error}"
    );
}

#[tokio::test]
async fn add_in_a_bound_prompt_with_empty_needs_fails_the_run_loudly() {
    // A bound prompt whose shared library declares nothing closes over empty
    // frozen bindings, so tools.add in a prologue is rejected the same way.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
```lua\nfunction helper() return 'no declarations' end\n```\n\n\
## Only\n\n```lua\ntools.add('web_search')\n```\n\nThis prose must not reach a model.\n";
    let error = run(&bound(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("an undeclared alias must fail the run");
    assert!(
        error.to_string().contains("not declared by tools.need"),
        "the error must report the missing declaration: {error}"
    );
}

#[tokio::test]
async fn prologue_return_skips_model_and_epilog() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\nreturn 'early'\n```\n\n\
This prose must not reach a model.\n\n\
```lua\nstore.write('epilog-ran.txt', 'yes')\nreturn 'late'\n```\n";
    let store = StoreRef::memory();
    let out = run(&bound(md), "", &[], &store, silent()).await.unwrap();

    assert_eq!(out, "early");
    assert!(store.read_lines("epilog-ran.txt").is_err());
}

#[tokio::test]
async fn shared_helper_survives_prologue_model_and_epilog() {
    let addr = spawn_text_gateway().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
```lua\nfunction decorate(value) return '<' .. value .. '>' end\n```\n\n\
## Only\n\n```lua\nvar.question = decorate(args)\n```\n\n\
Ask using {{ var.question }}.\n\n\
```lua\nreturn decorate(reply)\n```\n";
    let recorder = Recorder::default();
    let out = run(
        &bound_for_model(md),
        "input",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &recorder,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "<hello from the mock>");
    assert_eq!(
        recorder.events(),
        [
            ("Test prompt".to_owned(), detail::RUN_STARTED.to_owned()),
            ("Only".to_owned(), detail::SECTION_STARTED.to_owned()),
            (
                "Only".to_owned(),
                detail::LUA_SHARED_LOAD_STARTED.to_owned(),
            ),
            ("Only".to_owned(), detail::TOOL_REPLAY_STARTED.to_owned()),
            ("Only".to_owned(), detail::MODEL_REPLAY_STARTED.to_owned()),
            ("Only".to_owned(), detail::TOOL_REPLAY_SUCCEEDED.to_owned(),),
            ("Only".to_owned(), detail::MODEL_REPLAY_SUCCEEDED.to_owned()),
            (
                "Only".to_owned(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_owned(),
            ),
            ("Only".to_owned(), detail::LUA_PROLOGUE_STARTED.to_owned()),
            ("Only".to_owned(), detail::LUA_PROLOGUE_SUCCEEDED.to_owned(),),
            ("Only".to_owned(), detail::TOOL_SCOPE_CLOSING.to_owned()),
            ("Only".to_owned(), detail::TOOL_SCOPE_CLOSED.to_owned()),
            ("Only".to_owned(), detail::MODEL_SCOPE_CLOSING.to_owned()),
            ("Only".to_owned(), detail::MODEL_SCOPE_CLOSED.to_owned()),
            (
                "Only".to_owned(),
                detail::TOOL_SCOPE_VALIDATION_STARTED.to_owned(),
            ),
            (
                "Only".to_owned(),
                detail::TOOL_SCOPE_VALIDATION_SUCCEEDED.to_owned(),
            ),
            ("Only".to_owned(), detail::MODEL_TURN_COMPLETED.to_owned(),),
            (
                "Only".to_owned(),
                detail::LUA_REPLY_BINDING_STARTED.to_owned(),
            ),
            (
                "Only".to_owned(),
                detail::LUA_REPLY_BINDING_SUCCEEDED.to_owned(),
            ),
            ("Only".to_owned(), detail::LUA_EPILOG_STARTED.to_owned()),
            ("Only".to_owned(), detail::LUA_EPILOG_SUCCEEDED.to_owned(),),
            ("Only".to_owned(), detail::LUA_TEARDOWN_STARTED.to_owned()),
            ("Only".to_owned(), detail::LUA_TEARDOWN_SUCCEEDED.to_owned(),),
            ("Only".to_owned(), detail::SECTION_FINISHED.to_owned()),
            ("Test prompt".to_owned(), detail::RUN_SUCCEEDED.to_owned()),
        ]
    );
}

#[tokio::test]
async fn empty_prose_skips_model_but_runs_epilog_with_nil_reply() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\nvar.phase = 'prologue'\n```\n\n\
```lua\nif reply ~= nil then error('empty prose must not bind a reply') end\nreturn var.phase .. '-epilog'\n```\n";

    assert_eq!(
        run(&bound(md), "", &[], &StoreRef::memory(), silent())
            .await
            .unwrap(),
        "prologue-epilog"
    );
}

#[tokio::test]
async fn whitespace_only_prose_skips_model_without_binding() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\nif reply ~= nil then error('whitespace prose must not bind a reply') end\nreturn 'ok'\n```\n\n   \n\t\n";
    assert_eq!(
        run(&bound(md), "", &[], &StoreRef::memory(), silent())
            .await
            .unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn model_required_when_non_empty_prose_has_no_binding() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\nAsk the model.\n";
    let error = run(&bound(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("non-empty prose without a model binding must fail");
    assert!(
        matches!(error, Error::ModelRequired { .. }),
        "expected ModelRequired, got {error}"
    );
    assert!(
        error
            .to_string()
            .contains("model binding required for section Only"),
        "error must name the section: {error}"
    );
}

#[tokio::test]
async fn prologue_sys_model_unknown_during_shared_replay() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
```lua\nfunction read_sys_model()\n  return sys.model\nend\nmodels.always('writer', 'A general model for tests')\n```\n\n\
## Only\n\n```lua\nreturn read_sys_model()\n```\n\nprose\n";
    let error = run(&bound_for_model(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("H1 shared Lua must not read sys.model before scope close");
    assert!(
        error.to_string().contains("unknown sys field 'model'"),
        "error must name the missing field: {error}"
    );
}

#[tokio::test]
async fn prologue_sys_model_unknown_before_scope_close() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nreturn sys.model\n```\n\nprose\n";
    let error = run(&bound_for_model(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("prologue must not read sys.model before scope close");
    assert!(
        error.to_string().contains("unknown sys field 'model'"),
        "error must name the missing field: {error}"
    );
}

#[tokio::test]
async fn epilog_sees_sys_model_catalog_id_not_alias() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\n-- prologue\n```\n\n```lua\nreturn sys.model\n```\n\n";
    assert_eq!(
        run(&bound_for_model(md), "", &[], &StoreRef::memory(), silent())
            .await
            .unwrap(),
        "claude-sonnet-4-6"
    );
}

#[tokio::test]
async fn prose_substitution_sees_sys_model_catalog_id() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\n-- prologue\n```\n\nModel id is {{ sys.model }}.\n\n\
```lua\nreturn 'done'\n```\n";
    let (addr, captured) = spawn_capturing_text_gateway().await;
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out, "done");

    let body = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("complete must reach the gateway");
    let user_content = body["messages"]
        .as_array()
        .and_then(|messages| messages.first())
        .and_then(|message| message["content"].as_str())
        .expect("first message must carry substituted prose");
    assert!(
        user_content.contains("Model id is claude-sonnet-4-6."),
        "substituted prose must carry catalog id, got: {user_content}"
    );
}

#[tokio::test]
async fn empty_prose_epilog_sees_sys_model_when_binding_present() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\n-- prologue\n```\n\n```lua\nreturn sys.model\n```\n\n";
    assert_eq!(
        run(&bound_for_model(md), "", &[], &StoreRef::memory(), silent())
            .await
            .unwrap(),
        "claude-sonnet-4-6"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_epilog_sees_sys_model_catalog_id() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n\
## Parent\n\n```lua\nlocal r = fanout('### Worker', '### Items')\nreturn table.concat(r, ',')\n```\n\n\
### Worker\n\n```lua\n-- prologue\n```\n\nAsk about {{ item }}.\n\n\
```lua\nreturn sys.model .. ':' .. tostring(sys.reply_finish_reason) .. ':' .. item\n```\n\n\
### Items\n\n- a\n";
    let addr = spawn_text_finish_gateway("hello from the mock", "stop").await;
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out, "claude-sonnet-4-6:stop:a");
}

#[tokio::test]
async fn default_return_precedes_the_last_model_reply() {
    let addr = spawn_text_gateway().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\ndefault_return: fallback\n---\n\n\
# Test prompt\n\n\
## Only\n\nAsk the model.\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "fallback");
}

// --- Reply forwarding across sections ---

#[tokio::test]
async fn reply_carries_forward_to_next_section_prologue() {
    let addr = spawn_text_gateway().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\nAsk the model.\n\n\
## Second\n\n```lua\nreturn reply\n```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "hello from the mock");
}

#[tokio::test]
async fn reply_substitution_in_prose_uses_previous_section_reply() {
    let (addr, captured) = spawn_capturing_text_gateway().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## First\n\nAsk the model.\n\n\
## Second\n\nThe previous reply was: {{ reply }}\n\n\
```lua\nreturn reply\n```\n";
    run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    let body = captured
        .lock()
        .expect("capture lock")
        .take()
        .expect("must have captured");
    let messages = body["messages"].as_array().expect("messages array");
    let user_msg = messages.last().expect("last message");
    let content = user_msg["content"].as_str().expect("content string");
    assert!(
        content.contains("The previous reply was: hello from the mock"),
        "{{ reply }} must substitute the previous section's model text, got: {content}"
    );
}

#[tokio::test]
async fn reply_is_nil_in_first_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nreturn tostring(reply)\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "nil");
}

#[tokio::test]
async fn reply_substitution_nil_is_a_hard_error() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n{{ reply }}\n";
    let err = run_offline(md)
        .await
        .expect_err("{{ reply }} when nil must error");
    assert!(
        err.to_string().contains("reply"),
        "error must mention reply, got: {err}"
    );
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
    .with_annotations(ToolAnnotations {
        read_only: Some(true),
        destructive: Some(false),
        idempotent: Some(true),
    })
}

#[tokio::test]
async fn declared_tools_are_not_injected_without_always_or_add() {
    async fn completions(
        State(bodies): State<Arc<Mutex<Vec<Value>>>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        bodies.lock().unwrap().push(body);
        Json(json!({
            "choices": [{
                "message": { "role": "assistant", "content": "plain reply" }
            }]
        }))
    }

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&bodies));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let tool = ScopedFixtureTool::new("concrete", "canonical_wire", "Concrete description.");
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua\ntools.need('local_alias', 'capability')\nmodels.always('writer', 'A general model for tests')\n```\n\n\
## Only\n\nAsk without tools.\n";
    let prompt = bound_with_tools(
        md,
        &|_: &str| Ok(ToolId::new("tests", "concrete")),
        Vec::new(),
    );
    let out = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "plain reply");
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].get("tools").is_none(),
        "declaring a need must not expose it without explicit scope"
    );
}

#[tokio::test]
async fn always_advertises_concrete_schema_under_local_alias_and_dispatches_by_id() {
    let (addr, bodies, _) = spawn_aliased_tool_gateway("local_alias").await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Concrete description.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua\n\
tools.need('local_alias', 'capability')\n\
tools.always('local_alias')\n\
models.always('writer', 'A general model for tests')\n```\n\n\
## Only\n\nUse the tool.\n",
        &|_: &str| Ok(ToolId::new("tests", "concrete")),
        Vec::new(),
    );

    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let bodies = bodies.lock().unwrap();
    let function = &bodies[0]["tools"][0]["function"];
    assert_eq!(function["name"], "local_alias");
    assert_eq!(function["description"], "Concrete description.");
    assert_eq!(
        function["parameters"],
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"]
        })
    );
    assert_ne!(function["name"], "canonical_wire");
}

#[tokio::test]
async fn h2_add_scopes_an_alias_and_dispatches_the_concrete_tool() {
    let (addr, bodies, _) = spawn_aliased_tool_gateway("section_tool").await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Section concrete.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua\n\
tools.need('section_tool', 'capability')\n\
models.always('writer', 'A general model for tests')\n```\n\n\
## Only\n\n```lua\ntools.add('section_tool')\n```\n\nUse the tool.\n",
        &|_: &str| Ok(ToolId::new("tests", "concrete")),
        Vec::new(),
    );

    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        bodies.lock().unwrap()[0]["tools"][0]["function"]["name"],
        "section_tool"
    );
}

#[tokio::test]
async fn near_duplicate_tools_are_valid_when_isolated_in_separate_sections() {
    async fn completions(
        State(requests): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        requests.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "choices": [{"message": {"role": "assistant", "content": "text"}}]
        }))
    }

    let requests = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&requests));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let first = ScopedFixtureTool::new("first", "first_wire", "First concrete.");
    let second = ScopedFixtureTool::new("second", "second_wire", "Second concrete.");
    let first_descriptor = picker_descriptor("first", "Similar operation one.");
    let second_descriptor = picker_descriptor("second", "Similar operation two.");
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua\n\
tools.need('first_local', 'first')\n\
tools.need('second_local', 'second')\n\
models.always('writer', 'A general model for tests')\n```\n\n\
## First\n\n```lua\ntools.add('first_local')\n```\n\nFirst model turn.\n\n\
## Second\n\n```lua\ntools.add('second_local')\n```\n\nSecond model turn.\n",
        &|capability: &str| Ok(ToolId::new("tests", capability)),
        vec![NearDuplicate {
            first: first_descriptor,
            second: second_descriptor,
            similarity: 0.97,
        }],
    );

    let out = run(
        &prompt,
        "",
        &[
            Arc::new(first) as Arc<dyn Tool>,
            Arc::new(second) as Arc<dyn Tool>,
        ],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "text");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn near_duplicate_effective_scope_fails_before_the_model_without_payload_reports() {
    let (addr, _bodies, requests) = spawn_aliased_tool_gateway("first_local").await;
    let first = ScopedFixtureTool::new("first", "first_wire", "First concrete.");
    let second = ScopedFixtureTool::new("second", "second_wire", "Second concrete.");
    let first_descriptor = picker_descriptor("first", "Private similar description one.");
    let second_descriptor = picker_descriptor("second", "Private similar description two.");
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua\n\
tools.need('first_local', 'first')\n\
tools.need('second_local', 'second')\n\
models.always('writer', 'A general model for tests')\n```\n\n\
## Only\n\n```lua\ntools.add('first_local', 'second_local')\n```\n\nDo not reach the model.\n",
        &|capability: &str| Ok(ToolId::new("tests", capability)),
        vec![NearDuplicate {
            first: first_descriptor,
            second: second_descriptor,
            similarity: 0.98,
        }],
    );
    let recorder = Recorder::default();

    let error = run(
        &prompt,
        "",
        &[
            Arc::new(first) as Arc<dyn Tool>,
            Arc::new(second) as Arc<dyn Tool>,
        ],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &recorder,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(
        error,
        Error::NearDuplicateTools {
            diagnostic,
        } if diagnostic.first_alias == "first_local"
            && diagnostic.first_id == ToolId::new("tests", "first")
            && diagnostic.first_description == "Private similar description one."
            && diagnostic.first_annotations.read_only == Some(true)
            && diagnostic.second_alias == "second_local"
            && diagnostic.second_id == ToolId::new("tests", "second")
            && diagnostic.second_description == "Private similar description two."
            && diagnostic.second_annotations.idempotent == Some(true)
            && (diagnostic.similarity - 0.98).abs() < f32::EPSILON
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 0);
    let events = recorder.events();
    assert!(
        events
            .iter()
            .any(|(_, detail)| { detail == detail::TOOL_SCOPE_VALIDATION_FAILED })
    );
    assert!(!events.iter().any(|(_, detail)| {
        detail == detail::MODEL_TURN_COMPLETED || detail == detail::MODEL_TURN_FAILED
    }));
    let trace = format!("{events:?}");
    for payload in [
        "first_local",
        "second_local",
        "Private similar description",
        "Do not reach the model",
    ] {
        assert!(!trace.contains(payload), "observation leaked {payload:?}");
    }
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

#[tokio::test]
async fn debug_capture_receives_request_and_response_when_set() {
    let addr = spawn_text_gateway().await;
    let capture = RecordingCapture::default();
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\nAsk the model.\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: Some(&capture),
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "hello from the mock");
    let events = capture.events();
    assert_eq!(events.len(), 2, "one request and one response: {events:#?}");
    assert_eq!(events[0].0, EXECUTION);
    assert_eq!(events[0].1, "Only");
    assert_eq!(events[0].2, 1);
    match &events[0].3 {
        crate::debug::DebugEvent::Request { body } => {
            assert_eq!(body["model"], "claude-sonnet-4-6");
            assert!(body["messages"].as_array().is_some_and(|m| !m.is_empty()));
        }
        other => panic!("expected request first, got {other:?}"),
    }
    match &events[1].3 {
        crate::debug::DebugEvent::Response {
            body,
            finish_reason,
            reasoning_content,
        } => {
            assert_eq!(finish_reason, &None);
            assert_eq!(reasoning_content, &None);
            assert_eq!(
                body["choices"][0]["message"]["content"],
                "hello from the mock"
            );
        }
        other => panic!("expected response second, got {other:?}"),
    }
}

#[tokio::test]
async fn debug_capture_none_changes_nothing() {
    let addr = spawn_text_gateway().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\nAsk the model.\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");
}

// --- Per-VM tools.calls count tests ---

#[tokio::test]
async fn tool_calls_count_increments_on_successful_dispatch() {
    let (addr, _, _) = spawn_aliased_tool_gateway("echo").await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "echo",
        "canonical_echo",
        "Echo a test value.",
    ));
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua\n\
        tools.need('echo', 'echo tool')\n\
        tools.always('echo')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\nUse the tool.\n\n\
        ```lua\nassert(tools.calls['echo'] == 1, \
        'expected 1 call, got ' .. tostring(tools.calls['echo']))\n\
        return 'ok'\n```\n";
    let prompt = bound_with_tools(md, &|_: &str| Ok(ToolId::new("tests", "echo")), Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out, "ok");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tool_calls_count_increments_even_when_tool_errors() {
    let counts = crate::lua::ToolCallCounts::new(["echo".to_string()]);
    assert_eq!(counts.get("echo").unwrap(), Some(0));
    counts.increment("echo").unwrap();
    assert_eq!(counts.get("echo").unwrap(), Some(1));
    counts.increment("echo").unwrap();
    assert_eq!(counts.get("echo").unwrap(), Some(2));
}

#[tokio::test]
async fn tool_calls_count_zero_for_uncalled_alias_fails_epilog_assert() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua\n\
        tools.need('search', 'search tool')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n```lua\ntools.add('search')\n```\n\n\
        ```lua\nassert(tools.calls['search'] > 0, 'search was never called')\n\
        return 'unreached'\n```\n";
    let tool = ScopedFixtureTool::new("search", "canonical_search", "Search for things.");
    let addr = spawn_text_gateway().await;
    let prompt = bound_with_tools(
        md,
        &|_: &str| Ok(ToolId::new("tests", "search")),
        Vec::new(),
    );
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect_err("epilog assert on zero count must fail the run");
    assert!(
        error.to_string().contains("search was never called"),
        "error must carry the assert message: {error}"
    );
}

#[tokio::test]
async fn tool_calls_typo_alias_is_a_hard_error_with_in_scope_set() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua\n\
        tools.need('search', 'search tool')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n```lua\ntools.add('search')\n```\n\n\
        ```lua\nlocal _ = tools.calls['serach']\n\
        return 'unreached'\n```\n";
    let tool = ScopedFixtureTool::new("search", "canonical_search", "Search for things.");
    let addr = spawn_text_gateway().await;
    let prompt = bound_with_tools(
        md,
        &|_: &str| Ok(ToolId::new("tests", "search")),
        Vec::new(),
    );
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect_err("accessing a typo alias in tools.calls must hard error");
    let msg = error.to_string();
    assert!(
        msg.contains("serach") && msg.contains("not in this section's tool scope"),
        "error must name the bad key and state it's out of scope: {msg}"
    );
    assert!(
        msg.contains("search"),
        "error must list in-scope aliases: {msg}"
    );
}

#[tokio::test]
async fn model_calling_global_but_unscoped_tool_is_a_hard_error() {
    let (addr, _, _) = spawn_aliased_tool_gateway("global_tool").await;
    let scoped = ScopedFixtureTool::new("scoped", "canonical_scoped", "A scoped tool.");
    let global = ScopedFixtureTool::new("global_tool", "canonical_global", "A global tool.");
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua\n\
        tools.need('scoped', 'scoped tool')\n\
        tools.need('global_tool', 'global tool')\n\
        tools.always('scoped')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\nUse the tool.\n";
    let prompt = bound_with_tools(
        md,
        &|capability: &str| {
            if capability.contains("scoped") {
                Ok(ToolId::new("tests", "scoped"))
            } else {
                Ok(ToolId::new("tests", "global_tool"))
            }
        },
        Vec::new(),
    );
    let error = run(
        &prompt,
        "",
        &[
            Arc::new(scoped) as Arc<dyn Tool>,
            Arc::new(global) as Arc<dyn Tool>,
        ],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect_err("model calling a global-but-unscoped tool must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "global_tool");
            assert!(*global_exists, "the alias was declared by tools.need");
            assert!(
                in_scope.contains(&"scoped".to_string()),
                "in_scope must list the scoped alias: {in_scope:?}"
            );
            assert!(
                !in_scope.contains(&"global_tool".to_string()),
                "global_tool must not be in scope: {in_scope:?}"
            );
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    let msg = error.to_string();
    assert!(
        msg.contains("declared by tools.need but not added"),
        "error message must hint declared-but-unscoped: {msg}"
    );
}

#[tokio::test]
async fn model_calling_pure_unknown_tool_is_a_hard_error() {
    let (addr, _, _) = spawn_aliased_tool_gateway("nonexistent").await;
    let tool = ScopedFixtureTool::new("echo", "canonical_echo", "Echo a test value.");
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua\n\
        tools.need('echo', 'echo tool')\n\
        tools.always('echo')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\nUse the tool.\n";
    let prompt = bound_with_tools(md, &|_: &str| Ok(ToolId::new("tests", "echo")), Vec::new());
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect_err("model calling a pure unknown tool must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "nonexistent");
            assert!(
                !*global_exists,
                "the alias was never declared by tools.need"
            );
            assert!(
                in_scope.contains(&"echo".to_string()),
                "in_scope must list the scoped alias: {in_scope:?}"
            );
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    let msg = error.to_string();
    assert!(
        !msg.contains("declared by tools.need but not added"),
        "pure unknown must not hint declared-but-unscoped: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_infer_single_shot_returns_text() {
    let addr = spawn_mock_gateway().await;
    let echo = Arc::new(EchoTool);
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua\n\
        tools.need('echo', 'echo tool')\n\
        tools.always('echo')\n\
        writer = models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        local text = writer:infer('say hello')\n\
        assert(type(text) == 'string', 'infer must return text')\n\
        assert(text == 'final answer')\n\
        assert(reply == text, 'infer must set reply')\n\
        assert(tools.calls['echo'] == 1, 'infer tool loop must increment tools.calls')\n\
        return text\n\
        ```\n";
    let prompt = bound_with_tools(md, &|_: &str| Ok(ToolId::new("tests", "echo")), Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::clone(&echo) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect("model:infer single-shot must return text");
    assert_eq!(out, "final answer");
}

/// A second fixture tool so the bag can grow from one alias to two.
struct FetchTool;

#[async_trait::async_trait]
impl Tool for FetchTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "fetch")
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

    async fn call(&self, _args: Value) -> Result<String> {
        Ok("fetched".to_owned())
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "covers cache hit, generation bump rebuild, and count persistence in one bag lifecycle"
)]
fn tool_bag_caches_on_unchanged_generation() {
    let shared = LuaProgram::compile(
        "tools.need('echo', 'echo tool')\ntools.need('fetch', 'fetch tool')",
        "shared",
        1,
        EXECUTION,
        &NullObserver,
        "Bag",
    )
    .expect("shared Lua must compile");
    let bindings = bind_tool_declarations(
        &shared,
        &|description: &str| {
            if description.contains("echo") {
                Ok(ToolId::new("tests", "echo"))
            } else {
                Ok(ToolId::new("tests", "fetch"))
            }
        },
        EXECUTION,
        &NullObserver,
        "Bag",
    )
    .expect("declarations must bind");
    let mut vm = SectionVm::new_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Bag")
        .expect("declarations must replay");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let add_echo = LuaProgram::compile(
        "tools.add('echo')",
        "prologue",
        1,
        EXECUTION,
        &NullObserver,
        "Bag",
    )
    .expect("prologue must compile");
    vm.run_prologue(&add_echo, &NullObserver, "Bag")
        .expect("tools.add(echo) must succeed");

    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    {
        let runtime = tool_runtime.lock().expect("runtime mutex");
        assert_eq!(
            runtime.generation(),
            1,
            "first tools.add must bump generation"
        );
    }
    let mut bag = ToolBag::new(tool_bindings, Arc::clone(&tool_runtime));
    let echo = EchoTool;
    let fetch = FetchTool;
    let registry = ToolRegistry::new([&echo as &dyn Tool, &fetch as &dyn Tool]);

    let first = bag
        .prepare(None, &registry, EXECUTION, &NullObserver, "Bag")
        .expect("first prepare must build schemas");
    assert!(!first.reused, "first prepare must rebuild");
    assert_eq!(first.schemas.len(), 1);
    assert_eq!(first.schemas[0].name, "echo");

    let second = bag
        .prepare(None, &registry, EXECUTION, &NullObserver, "Bag")
        .expect("second prepare must reuse cache");
    assert!(second.reused, "unchanged generation must reuse cache");
    assert_eq!(
        second
            .schemas
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<Vec<_>>(),
        first
            .schemas
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<Vec<_>>(),
    );
    assert_eq!(second.dispatch, first.dispatch);

    let add_fetch = LuaProgram::compile(
        "tools.add('fetch')",
        "prologue-2",
        1,
        EXECUTION,
        &NullObserver,
        "Bag",
    )
    .expect("second prologue must compile");
    vm.run_prologue(&add_fetch, &NullObserver, "Bag")
        .expect("tools.add(fetch) must succeed");
    {
        let runtime = tool_runtime.lock().expect("runtime mutex");
        assert_eq!(
            runtime.generation(),
            2,
            "second tools.add must bump generation"
        );
    }

    let third = bag
        .prepare(None, &registry, EXECUTION, &NullObserver, "Bag")
        .expect("prepare after mutation must rebuild");
    assert!(!third.reused, "generation mismatch must rebuild");
    assert_eq!(third.schemas.len(), 2);
    assert_eq!(
        third
            .schemas
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<Vec<_>>(),
        ["echo", "fetch"]
    );

    // Counts persist across prepare/infer; new tools seed at 0.
    let counts = ToolCallCounts::new(first.scope.bindings().iter().map(|b| b.alias().to_owned()));
    counts.increment("echo").expect("echo must be seeded");
    assert_eq!(counts.get("echo").unwrap(), Some(1));
    counts.ensure("fetch").expect("new tool seeds at 0");
    assert_eq!(counts.get("fetch").unwrap(), Some(0));
    assert_eq!(
        counts.get("echo").unwrap(),
        Some(1),
        "existing counts must persist when new tools are seeded"
    );

    vm.teardown(&NullObserver, "Bag");
}

#[test]
fn tool_description_override_appears_in_model_schema() {
    let shared = LuaProgram::compile(
        "echo = tools.need('echo', 'echo capability for bind matching')",
        "shared",
        1,
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("shared Lua must compile");
    let bindings = bind_tool_declarations(
        &shared,
        &|_: &str| Ok(ToolId::new("tests", "echo")),
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("declarations must bind");
    let echo = EchoTool;
    let registry = ToolRegistry::new([&echo as &dyn Tool]);

    // tools.add(Tool) without mutating .description keeps the registry text.
    let mut default_vm =
        SectionVm::new_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Override")
            .expect("declarations must replay");
    default_vm
        .inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let add_default = LuaProgram::compile(
        "tools.add(echo)",
        "prologue",
        1,
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("prologue must compile");
    default_vm
        .run_prologue(&add_default, &NullObserver, "Override")
        .expect("tools.add(echo) without override must succeed");
    let (default_bindings, default_runtime) = default_vm.tool_bag_handles();
    let mut default_bag = ToolBag::new(default_bindings, Arc::clone(&default_runtime));
    let default_prepared = default_bag
        .prepare(None, &registry, EXECUTION, &NullObserver, "Override")
        .expect("default prepare must build schemas");
    assert_eq!(default_prepared.schemas.len(), 1);
    assert_eq!(
        default_prepared.schemas[0].description,
        echo.description(),
        "unmutated Tool object must still advertise the registry description"
    );
    assert_eq!(
        default_prepared.scope.bindings()[0].description(),
        "echo capability for bind matching",
        "bind capability text must stay on the binding"
    );
    assert_eq!(
        default_prepared.scope.bindings()[0].model_description(),
        None
    );
    default_vm.teardown(&NullObserver, "Override");

    // Mutating .description before tools.add overrides the model-facing schema.
    let mut vm =
        SectionVm::new_with_bindings(&shared, &bindings, EXECUTION, &NullObserver, "Override")
            .expect("declarations must replay");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let add_override = LuaProgram::compile(
        "echo.description = 'Author override for the model'\n\
         assert(echo.description == 'Author override for the model')\n\
         tools.add(echo)",
        "prologue",
        1,
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("prologue must compile");
    vm.run_prologue(&add_override, &NullObserver, "Override")
        .expect("description override before tools.add must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    let mut bag = ToolBag::new(tool_bindings, Arc::clone(&tool_runtime));
    let prepared = bag
        .prepare(None, &registry, EXECUTION, &NullObserver, "Override")
        .expect("override prepare must build schemas");
    assert_eq!(prepared.schemas.len(), 1);
    assert_eq!(
        prepared.schemas[0].description,
        "Author override for the model"
    );
    assert_eq!(
        prepared.scope.bindings()[0].description(),
        "echo capability for bind matching",
        "override must not rewrite the bind capability description"
    );
    assert_eq!(
        prepared.scope.bindings()[0].model_description(),
        Some("Author override for the model")
    );

    vm.teardown(&NullObserver, "Override");
}

/// Alternating lua/prose blocks run in order; non-final prose is single-shot,
/// final prose loops, and trailing lua sees the last reply.
#[tokio::test]
async fn section_with_alternating_blocks_executes_in_order() {
    async fn completions(
        State(calls): State<Arc<AtomicU32>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        let n = calls.fetch_add(1, Ordering::Relaxed) + 1;
        Json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": format!("reply-{n}")
                }
            }]
        }))
    }

    let calls = Arc::new(AtomicU32::new(0));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n\
```lua\nstore.append('order.txt', 'lua1\\n')\n```\n\n\
First ask.\n\n\
```lua\nstore.append('order.txt', 'lua2\\n')\n```\n\n\
Final ask.\n\n\
```lua\nstore.append('order.txt', 'lua3\\n')\nreturn reply\n```\n";
    let store = StoreRef::memory();
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect("alternating blocks must execute");

    assert_eq!(out, "reply-2");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        store.read_lines("order.txt").expect("order log"),
        "1| lua1\n2| lua2\n3| lua3"
    );

    let section = parse(md).entry().clone();
    assert_eq!(section.blocks.len(), 5);
    assert!(matches!(
        &section.blocks[1],
        crate::parser::Block::Prose {
            loop_capable: false,
            ..
        }
    ));
    assert!(matches!(
        &section.blocks[3],
        crate::parser::Block::Prose {
            loop_capable: true,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_runs_named_section_as_subroutine() {
    async fn completions(Json(_body): Json<Value>) -> Json<Value> {
        Json(json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "research-reply"
                }
            }]
        }))
    }

    let router = Router::new().route("/v1/chat/completions", post(completions));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Research\n\n\
```lua\n\
local step = tasks['## Research']\n\
assert(step.name == 'Research')\n\
assert(step.has_prose == true)\n\
```\n\n\
Research {{ args }}.\n\n\
```lua\nstore.write('evidence.md', reply)\n```\n\n\
## Main\n\n\
```lua\n\
local research = tasks['## Research']\n\
assert(research.name == 'Research')\n\
assert(research.has_prose == true)\n\
local by_name = execute('## Research')\n\
local by_obj = execute(research)\n\
assert(by_name == 'research-reply')\n\
assert(by_obj == 'research-reply')\n\
assert(store.read('evidence.md') == 'research-reply')\n\
return by_name\n\
```\n";
    let store = StoreRef::memory();
    let out = run(
        &bound_for_model(md),
        "topic",
        &[],
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect("execute must run named section as subroutine");
    assert_eq!(out, "research-reply");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goto_transfers_control_and_clears_context() {
    // Lua 5.4 reserves `goto` as a keyword, so the host installs the transfer
    // function under _G["goto"] and authors call it through that index.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Check\n\n\
```lua\n\
store.write('seen.txt', 'check')\n\
local help = tasks['## Help']\n\
assert(help.name == 'Help')\n\
assert(help.has_prose == false)\n\
_G['goto'](help)\n\
store.write('seen.txt', 'should-not-run')\n\
```\n\n\
## Accept\n\n\
```lua\nreturn 'accepted'\n```\n\n\
## Help\n\n\
```lua\n\
assert(reply == nil, 'goto must clear prior reply context')\n\
return 'helped:' .. store.read('seen.txt')\n\
```\n";
    let store = StoreRef::memory();
    let out = run(&bound(md), "", &[], &store, silent())
        .await
        .expect("goto must transfer control");
    assert_eq!(out, "helped:check");
    assert_eq!(store.read("seen.txt").expect("seen"), "check");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_returns_structured_results() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', '### Items')\n\
assert(r[1].text == 'alpha-1')\n\
assert(r[1].ok == true)\n\
assert(r[1].item == 'alpha')\n\
assert(r[1].exhausted == false)\n\
assert(r[2].text == 'beta-2')\n\
assert(r[2].ok == true)\n\
assert(r[2].item == 'beta')\n\
assert(r[2].exhausted == false)\n\
assert(tostring(r[1]) == r[1].text)\n\
assert(table.concat(r, ',') == 'alpha-1,beta-2')\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\nreturn item .. '-' .. sys.taskid\n```\n\n\
Do work.\n\n\
### Items\n\n\
- alpha\n\
- beta\n";
    let out = run(&bound(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect("fanout must return structured results");
    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_exhausted_arm_exposes_failure_metadata() {
    let (addr, _) = spawn_always_tool_call().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\nmax_tool_iterations: 2\n---\n\n\
# Test prompt\n\n```lua\n\
tools.need('echo', 'echo tool')\n\
models.always('writer', 'A general model for tests')\n```\n\n\
## Parent\n\n\
```lua\n\
local r = fanout('### Worker', '### Items')\n\
assert(r[1].ok == false)\n\
assert(r[1].exhausted == true)\n\
assert(r[1].item == 'alpha')\n\
assert(r[1].text:find('tool loop exhausted', 1, true))\n\
assert(tostring(r[1]) == r[1].text)\n\
return 'ok'\n\
```\n\n\
### Worker\n\n\
```lua\ntools.add('echo')\n```\n\n\
Loop forever on {{ item }}.\n\n\
### Items\n\n\
- alpha\n";
    let prompt = bound_with_tools(md, &|_: &str| Ok(ToolId::new("tests", "echo")), Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::new(EchoTool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect("soft-degraded fanout must still return structured results");
    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sys_exposes_section_metadata() {
    let addr = spawn_text_finish_gateway("alpha-answer", "stop").await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Alpha\n\n\
```lua\n\
assert(sys.section_name == 'Alpha')\n\
assert(sys.execution == 'execute-test')\n\
assert(sys.section_count == 2)\n\
local ok = pcall(function() return sys.reply_finish_reason end)\n\
assert(not ok, 'reply_finish_reason must be absent before prose')\n\
```\n\n\
Write one fact.\n\n\
```lua\n\
assert(sys.reply_finish_reason == 'stop')\n\
assert(reply == 'alpha-answer')\n\
```\n\n\
## Beta\n\n\
```lua\n\
assert(sys.section_name == 'Beta')\n\
assert(sys.section_count == 2)\n\
assert(sys.execution == 'execute-test')\n\
return 'done'\n\
```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: &NullObserver,
            client: Some(GatewayClient::new(&format!("http://{addr}/v1"), "test")),
            debug: None,
        },
    )
    .await
    .expect("sys must expose section metadata");
    assert_eq!(out, "done");
}
