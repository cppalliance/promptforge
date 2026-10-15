//! Shared context builders, prompt sources, parsers, stores, and recorders
//! for the execution suites. The fixture tool-set helpers sit in `tools`
//! and are re-exported here.

use promptforge_parser::test_support::PromptSource;

use super::*;

#[path = "context-tools.rs"]
mod tools;

pub(super) use self::tools::*;
pub(super) use crate::test_support::scenario::{
    EXECUTION, TEST_MODEL, TEST_SEED, TEST_STARTED_AT, TestStore, test_model,
};

/// Declares the `writer` role in `source`'s frontmatter and parks it as the
/// prompt-wide default in its H1, so a bound scenario runs its sections
/// under the test model.
pub(super) fn with_writer(source: PromptSource) -> PromptSource {
    source
        .front("models: { writer: {} }")
        .h1("models.default('writer')")
}

/// The prompt `source` builds, declaring the `writer` role and requiring
/// `## setup` from its H1: the library, after `source`'s sections, that
/// parks the role as the prompt-wide default.
pub(super) fn setup_prompt(source: PromptSource) -> String {
    source
        .front("models: { writer: {} }")
        .h1("require '## setup'")
        .section("setup", "models.default('writer')")
        .build()
}

/// The scaffold the fanout-arm capability tests share: a `## Parent` that
/// fans `### Worker` out over one `alpha` member and returns the first arm's
/// text. The worker body and its sibling sections follow.
pub(super) fn arm_fanout() -> PromptSource {
    PromptSource::new().section(
        "Parent",
        "
        local r = tasks.fanout('### Worker', {'alpha'})
        return r[1].text",
    )
}

/// The context edit binding the role `label` to `model` by hand, for a run
/// that skips the prepare.
pub(super) fn bind_role(
    label: &str,
    model: ModelDescriptor,
) -> impl FnOnce(RunContext) -> RunContext + 'static {
    let label = label.to_owned();
    move |mut ctx| {
        ctx.model_bindings.bind(&label, model);
        ctx
    }
}

/// A fresh default handle's access capability, for tests that inject Engine
/// values into a standalone VM.
pub(super) fn fresh_access() -> Arc<Access> {
    Arc::new(
        VfsRef::default()
            .acquire(promptforge_vfs::Origin::new("execute test fixture"))
            .expect("the stock backend acquires"),
    )
}

/// A [`RunContext`] for the run `name` under the fixed test inputs.
pub(super) fn test_context(name: impl Into<String>) -> RunContext {
    RunContext::new(name, TEST_SEED, TEST_STARTED_AT)
}

/// Compile-time proof that the public execution types are thread-safe.
///
/// `RunContext` holds `Arc<dyn Observer>` / `Arc<dyn DebugCapture>` (shared
/// trait objects) and must be `Send + Sync + 'static` to cross the run's task
/// boundaries; the typed error/limit/result surfaces must be too.
const fn _public_execution_types_are_send_sync_static() {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync_static::<RunContext>();
    assert_send_sync_static::<RunLimits>();
    assert_send_sync_static::<RunResult>();
    assert_send_sync_static::<RunError>();
    assert_send_sync_static::<RunErrorKind>();
}

/// The runtime's default per-section tool-loop cap, mirrored for tests after the
/// `DEFAULT_MAX_TOOL_ITERATIONS` constant was folded into `RunLimits`.
pub(super) const DEFAULT_MAX_TOOL_ITERATIONS: usize = 24;

/// The `writer` role declaration every model-facing fixture prompt includes:
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
pub(super) fn parse(md: &str) -> Prompt {
    let source = if md.lines().any(|line| line.starts_with("# ")) {
        md.to_string()
    } else {
        md.replacen("---\n\n", "---\n\n# Test prompt\n\n", 1)
    };
    Prompt::parse(&source, EXECUTION).0.unwrap()
}

pub(super) struct TestPrompt {
    pub(super) prompt: Prompt,
    pub(super) models: ModelCatalog,
}

/// Builds the tool-free parsed form consumed by the complete lifecycle path.
pub(super) fn fixture(md: &str) -> TestPrompt {
    TestPrompt {
        prompt: parse(md),
        models: ModelCatalog::empty(),
    }
}

#[track_caller]
pub(super) fn test_model_catalog() -> ModelCatalog {
    ModelCatalog::new([test_model(TEST_MODEL)]).expect("the test catalog has a single unique model")
}

/// Declares the `writer` role and parks it as the prompt-wide default, so a
/// model-facing fixture prompt runs its sections under a bound model.
/// Prompts with their own `models.default` call (or a `models.bind` call,
/// which the tests expect to fail) keep their shape and get only the role
/// declaration.
fn ensure_model_h1(md: &str) -> String {
    let source = md.to_string();
    if source.contains("models.default") || source.contains("models.bind") {
        return declare_writer(&source);
    }
    let mut source = declare_writer(&source);
    if let Some(pos) = source.find("\n\n## ") {
        source.insert_str(pos + 2, MODEL_DEFAULT_H1);
        return source;
    }
    source.replacen(
        "---\n\n",
        &format!("---\n\n# Test prompt\n\n{MODEL_DEFAULT_H1}"),
        1,
    )
}

pub(super) fn bound_for_model(md: &str) -> TestPrompt {
    TestPrompt {
        prompt: parse(&ensure_model_h1(md)),
        models: test_model_catalog(),
    }
}

// The helper takes no resolver: live tool binding resolves elsewhere, so a
// resolver argument would imply a resolution path the helper does not
// exercise. The fixture Plugin's tools are in the run's catalog.
pub(super) fn bound_with_tools(md: &str) -> TestPrompt {
    let source = ensure_model_h1(md);
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
/// and optional client/capture sinks. The instrumentation is owned `Arc`s
/// so it can build a [`RunContext`].
pub(super) struct RunOptions {
    pub(super) execution: &'static str,
    pub(super) observer: Arc<dyn Observer>,
    pub(super) client: Option<ScriptedChat>,
    pub(super) debug: Option<Arc<dyn DebugCapture>>,
}

/// Options that report nowhere and build no client - what a Lua-only,
/// offline test wants.
pub(super) fn silent() -> RunOptions {
    RunOptions {
        execution: EXECUTION,
        observer: Arc::new(NullObserver::default()),
        client: None,
        debug: None,
    }
}

/// A client answering from the given scripted model, sharing its script,
/// count, and record.
pub(super) fn gateway_client(chat: &ScriptedChat) -> ScriptedChat {
    chat.clone()
}

/// Options that report nowhere and answer the run's rounds from the given
/// scripted model.
pub(super) fn gatewayed(chat: &ScriptedChat) -> RunOptions {
    RunOptions {
        execution: EXECUTION,
        observer: Arc::new(NullObserver::default()),
        client: Some(gateway_client(chat)),
        debug: None,
    }
}

/// Options that answer from a scripted model and record debug events.
pub(super) fn gatewayed_with_debug(
    chat: &ScriptedChat,
    capture: Arc<dyn DebugCapture>,
) -> RunOptions {
    RunOptions {
        debug: Some(capture),
        ..gatewayed(chat)
    }
}

/// Parses `md` and runs it offline with empty `args`, no tools, and a fresh
/// in-memory store created for the run - the ergonomic path for the
/// Lua-only tests that do not care about the store's contents.
pub(super) async fn run_offline(md: &str) -> Result<String> {
    run(&fixture(md), "", &[], &TestStore::new(), silent()).await
}

pub(super) async fn run(
    test: &TestPrompt,
    args: &str,
    tools: &[Arc<dyn TestTool>],
    store: &TestStore,
    opts: RunOptions,
) -> Result<String> {
    let mut fixture = RunFixture::new().observer(opts.observer);
    // The test store's handle is the run's whole filesystem: the context
    // takes it as given, so post-run assertions read what the run
    // actually wrote.
    let mut ctx = test_context(opts.execution).vfs(store.vfs());
    if !tools.is_empty() {
        // With tools: the fixtures' descriptors form the catalog the run
        // offers, and the implementations go to the tool table the test
        // driver's tool performer resolves a `ToolCall` effect in.
        let (catalog, table) = fixture_tools(tools);
        ctx = ctx.tools(catalog);
        fixture = fixture.tools(table);
    }
    // The context holds the current model, and
    // prepare's trivial fill binds every declared role to it.
    if let Some(model) = test.models.models().first() {
        ctx = ctx.model(model.clone());
    }
    if let Some(client) = opts.client {
        fixture = fixture.client(client);
    }
    if let Some(debug) = opts.debug {
        ctx = ctx.report_debug(promptforge_types::emitter::DebugMode::On);
        fixture = fixture.debug(debug);
    }
    match crate::test_support::run_with_fixture(&test.prompt, args, ctx, fixture).await {
        RunResult::Ok(output) => Ok(output),
        RunResult::Cancelled => Err(Error::Interrupted),
        RunResult::Failure(error) => Err(Error::from(error)),
    }
}

/// Runs a fixture offline through the test-support driver
/// ([`crate::test_support::run_with_fixture`]) with a caller-customized
/// [`RunContext`] and [`RunFixture`], returning the typed [`RunError`]
/// so a test can assert on its kind (limits, cancellation).
pub(super) async fn run_with_context(
    test: &TestPrompt,
    configure: impl FnOnce(RunContext, RunFixture) -> (RunContext, RunFixture),
) -> std::result::Result<String, RunError> {
    let (mut ctx, fixture) = configure(test_context(EXECUTION), RunFixture::new());
    ctx = ctx.vfs(TestStore::new().vfs());
    if ctx.model.is_none()
        && let Some(model) = test.models.models().first()
    {
        ctx = ctx.model(model.clone());
    }
    match crate::test_support::run_with_fixture(&test.prompt, "", ctx, fixture).await {
        RunResult::Ok(output) => Ok(output),
        RunResult::Cancelled => Err(RunError::from(Error::Interrupted)),
        RunResult::Failure(error) => Err(error),
    }
}

/// An [`Observer`] that keeps every observation it is handed, in order, so a test
/// can assert on the whole sequence rather than on a count.
#[derive(Default)]
pub(super) struct Recorder(Mutex<Vec<(String, String, String)>>);

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
    pub(super) fn records(&self) -> Vec<(String, String, String)> {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .clone()
    }

    /// The observations recorded so far, in order.
    pub(super) fn events(&self) -> Vec<(String, String)> {
        self.0
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .iter()
            .map(|(_, section, detail)| (section.clone(), detail.clone()))
            .collect()
    }
}

/// Runs `md` offline under a fresh recorder and returns the result together
/// with every complete correlated record the recorder saw.
pub(super) async fn run_recorded(md: &str) -> (Result<String>, Vec<(String, String, String)>) {
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

/// Discards only the execution field when an older ordering regression is
/// intentionally about section and detail rather than correlation.
pub(super) fn events(records: &[(String, String, String)]) -> Vec<(String, String)> {
    records
        .iter()
        .map(|(_, section, detail)| (section.clone(), detail.clone()))
        .collect()
}
