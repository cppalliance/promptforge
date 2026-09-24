//! Shared context builders, parsers, stores, and recorders for the
//! execution suites.

use super::*;

/// A fresh stock handle's access capability, for tests that inject host
/// values into a standalone VM.
pub(super) fn fresh_access() -> Arc<Access> {
    Arc::new(
        promptforge_vfs::empty()
            .acquire(promptforge_vfs::Origin::new("execute test fixture"))
            .expect("the stock backend acquires"),
    )
}

pub(super) const EXECUTION: &str = "execute-test";

/// The fixed host inputs every test run shares: a seed and a start instant
/// a test that does not care about them never has to choose. The tests of
/// the inputs themselves (`run_inputs`) build their contexts directly.
pub(super) const TEST_SEED: u64 = 1;
pub(super) const TEST_STARTED_AT: promptforge_types::timestamp::Timestamp =
    promptforge_types::timestamp::Timestamp::from_unix_millis(1_700_000_000_000);

/// A [`RunContext`] for the run `name` under the fixed test inputs.
pub(super) fn test_context(name: impl Into<String>) -> RunContext {
    RunContext::new(name, TEST_SEED, TEST_STARTED_AT)
}

/// F10: compile-time proof that the public execution types are thread-safe.
///
/// `RunContext` holds `Arc<dyn Observer>` / `Arc<dyn DebugCapture>` (shared
/// trait objects) and must be `Send + Sync + 'static` to cross the run's task
/// boundaries; the typed error/limit/result surfaces and the environment must
/// be too.
pub(super) const fn _public_execution_types_are_send_sync_static() {
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
pub(super) const DEFAULT_MAX_TOOL_ITERATIONS: usize = 24;

/// The `writer` role declaration every model-facing fixture prompt includes:
/// the frontmatter slot, filled by prepare's trivial fill from the
/// context's current model.
pub(super) const MODEL_ROLE_DECL: &str = "models:\n  writer: {}\n";

/// The H1 block parking the declared role as the prompt-wide default.
pub(super) const MODEL_DEFAULT_H1: &str = "```lua\nmodels.default('writer')\n```\n\n";

/// Declares the `writer` role in the prompt's frontmatter, unless the
/// frontmatter already declares roles.
pub(super) fn declare_writer(source: &str) -> String {
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

impl TestPrompt {
    pub(super) fn prompt(&self) -> &Prompt {
        &self.prompt
    }
}

/// Builds the tool-free parsed form consumed by the complete lifecycle path.
pub(super) fn fixture(md: &str) -> TestPrompt {
    TestPrompt {
        prompt: parse(md),
        models: ModelCatalog::empty(),
    }
}

pub(super) fn test_model_catalog() -> ModelCatalog {
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
/// Prompts with their own `models.default` call (or the legacy
/// `models.bind` of the removal tests) keep their shape and get only the
/// role declaration.
pub(super) fn ensure_model_h1(md: &str) -> String {
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

pub(super) fn bound_for_model(md: &str) -> TestPrompt {
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
pub(super) fn bound_with_tools(md: &str) -> TestPrompt {
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
pub(super) struct RunOptions {
    pub(super) execution: &'static str,
    pub(super) observer: Arc<dyn Observer>,
    pub(super) client: Option<MockGatewayClient>,
    pub(super) debug: Option<Arc<dyn DebugCapture>>,
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
pub(super) struct TestStore(Mutex<VfsRef>);

impl TestStore {
    pub(super) fn new() -> TestStore {
        TestStore(Mutex::new(promptforge_vfs::empty()))
    }

    /// Wraps a caller-built handle - a gated backend, say - in the test
    /// store's seeding and post-run assertion helpers.
    pub(super) fn from_vfs(vfs: VfsRef) -> TestStore {
        TestStore(Mutex::new(vfs))
    }

    /// The handle the run and the context builders take.
    pub(super) fn vfs(&self) -> VfsRef {
        self.0
            .lock()
            .expect("the store lock is not poisoned")
            .clone()
    }

    /// Points the store at the run's prepared handle, so post-run
    /// assertions read the store the run actually used.
    pub(super) fn reconnect(&self, vfs: VfsRef) {
        *self.0.lock().expect("the store lock is not poisoned") = vfs;
    }

    pub(super) fn read(&self, path: &str) -> std::result::Result<String, StoreError> {
        let vfs = self.vfs();
        let access = vfs
            .acquire(promptforge_vfs::Origin::new("TestStore::read"))
            .map_err(StoreError::backend)?;
        vfs.store(&access).read(path)
    }

    pub(super) fn glob(&self, pattern: &str) -> std::result::Result<Vec<String>, StoreError> {
        let vfs = self.vfs();
        let access = vfs
            .acquire(promptforge_vfs::Origin::new("TestStore::glob"))
            .map_err(StoreError::backend)?;
        vfs.store(&access).glob(pattern)
    }
}

/// Builds a [`RunContext`] and its [`RunHost`] from the test-local
/// [`RunOptions`], for the tests that call [`Environment::run`] directly.
/// The context sets the test model as the current selection, so prepare's
/// trivial fill binds every declared role to it; the observer, client, and
/// capture go on the host the driver performs and reports through.
pub(super) fn to_context(opts: RunOptions) -> (RunContext, RunHost) {
    let mut ctx = test_context(opts.execution).model(test_model_catalog().models()[0].clone());
    let mut host = RunHost::new().observer(opts.observer);
    if let Some(client) = opts.client {
        host = host.client(client);
    }
    if let Some(debug) = opts.debug {
        ctx = ctx.report_debug(promptforge_types::emitter::DebugMode::On);
        host = host.debug(debug);
    }
    (ctx, host)
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

/// Builds a client pointed at the given scripted gateway.
pub(super) fn gateway_client(addr: SocketAddr) -> MockGatewayClient {
    MockGatewayClient::new(addr, "test")
}

/// Options that report nowhere and point the run's client at the given
/// scripted gateway.
pub(super) fn gatewayed(addr: SocketAddr) -> RunOptions {
    RunOptions {
        execution: EXECUTION,
        observer: Arc::new(NullObserver::default()),
        client: Some(gateway_client(addr)),
        debug: None,
    }
}

/// Options that point at a scripted gateway and record debug events.
pub(super) fn gatewayed_with_debug(addr: SocketAddr, capture: Arc<dyn DebugCapture>) -> RunOptions {
    RunOptions {
        debug: Some(capture),
        ..gatewayed(addr)
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
    let mut env = Environment::new();
    let mut host = RunHost::new().observer(opts.observer);
    // The run's own router (a fresh store backend per run) is built here
    // and set on the context, so the test store can reconnect to the
    // handle the run will use and read back what the run actually wrote.
    let vfs = env.run_vfs();
    store.reconnect(vfs.clone());
    let mut ctx = test_context(opts.execution).vfs(vfs);
    if !tools.is_empty() {
        // The host pattern with tools: the fixtures' descriptors form the
        // catalog the run binds its frontmatter slots against, and the
        // implementations go to the host table the driver's tool
        // performer resolves a `ToolCall` effect in - the two halves a
        // harness assembles from its activated capabilities.
        let (catalog, table) = fixture_tools(tools);
        env = env.tools(catalog);
        host = host.tools(table);
    }
    // The host pattern: the context holds the current model, and
    // prepare's trivial fill binds every declared role to it.
    if let Some(model) = test.models.models().first() {
        ctx = ctx.model(model.clone());
    }
    if let Some(client) = opts.client {
        host = host.client(client);
    }
    if let Some(debug) = opts.debug {
        ctx = ctx.report_debug(promptforge_types::emitter::DebugMode::On);
        host = host.debug(debug);
    }
    match crate::test_support::run_with_host(&env, &test.prompt, args, ctx, host).await {
        RunResult::Ok(output) => Ok(output),
        RunResult::Cancelled => Err(Error::Interrupted),
        RunResult::Failure(error) => Err(Error::from(error)),
    }
}

/// A binding for a fixture tool beside its implementation: the binding
/// goes into the run's tool set, the implementation into the host table
/// [`arm_tools`] hands the driver, so a script or model call on the alias
/// resolves through the same id the binding journals.
pub(super) fn fixture_binding(
    alias: &str,
    description: &str,
    tool: Arc<dyn TestTool>,
) -> (crate::lua::ToolBinding, Arc<dyn TestTool>) {
    let binding = crate::lua::ToolBinding::for_test(alias, description, &tool.descriptor());
    (binding, tool)
}

/// A run's tool set beside the implementations behind it: the set goes to
/// the run state (what the engine advertises and journals), the table to
/// the state's test host (what the driver performs a `ToolCall` with).
/// A bare [`ToolSet`](crate::lua::ToolSet) converts into a fixture with no
/// implementations, for the tests whose tools are never called.
#[derive(Clone, Default)]
pub(super) struct FixtureTools {
    set: crate::lua::ToolSet,
    table: TestToolTable,
}

impl FixtureTools {
    /// Builds the fixture from bindings paired with their implementations
    /// and the prompt-wide `always` aliases.
    pub(super) fn new(
        bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
        always: Vec<String>,
    ) -> Self {
        let mut table = TestToolTable::new();
        let bindings = bindings
            .into_iter()
            .map(|(binding, tool)| {
                table.insert(tool);
                binding
            })
            .collect();
        Self {
            set: crate::lua::ToolSet::for_test(bindings, always),
            table,
        }
    }

    /// The bindings as the run's set, for a test that inspects them.
    pub(super) fn set(&self) -> &crate::lua::ToolSet {
        &self.set
    }

    /// Installs the set on the run state and returns `host` carrying the
    /// implementations the driver's tool performer resolves.
    pub(super) fn install(&self, ctx: &RunState, host: RunHost) -> RunHost {
        *ctx.tool_set()
            .lock()
            .expect("the tool set mutex is not poisoned") = self.set.clone();
        host.tools(self.table.clone())
    }
}

impl From<crate::lua::ToolSet> for FixtureTools {
    fn from(set: crate::lua::ToolSet) -> Self {
        Self {
            set,
            table: TestToolTable::new(),
        }
    }
}

/// Arms the run state's shared tool set with `bindings` (every alias
/// prompt-wide through `always`) and returns `host` carrying the
/// implementations, so `TokioDriver::new` performs the calls.
pub(super) fn arm_tools(
    ctx: &RunState,
    host: RunHost,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
) -> RunHost {
    let always = bindings
        .iter()
        .map(|(binding, _)| binding.alias().to_owned())
        .collect();
    arm_tools_scoped(ctx, host, bindings, always)
}

/// Arms the run state's shared tool set with `bindings` and exactly
/// `always` as the prompt-wide scope, returning `host` carrying the
/// implementations.
pub(super) fn arm_tools_scoped(
    ctx: &RunState,
    host: RunHost,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
    always: Vec<String>,
) -> RunHost {
    FixtureTools::new(bindings, always).install(ctx, host)
}

/// The test's tools as the two halves a host assembles from its
/// activated capabilities: the catalog of descriptors the run's
/// frontmatter tool slots (under `tests/tools`) fill against at prepare,
/// and the table of implementations the driver's tool performer resolves
/// a `ToolCall` effect's id in.
pub(super) fn fixture_tools(
    tools: &[Arc<dyn TestTool>],
) -> (promptforge_types::tools::ToolCatalog, TestToolTable) {
    let table = TestToolTable::from_tools(tools);
    let catalog = table
        .catalog()
        .expect("the fixture tools have legal wire names and distinct ids");
    (catalog, table)
}

/// The test-support driver ([`crate::test_support::run_with_host`]) with the
/// context and host [`to_context`] assembled (observer, client, capture).
pub(super) async fn env_run(
    env: &Environment,
    prompt: &Prompt,
    args: &str,
    prepared: (RunContext, RunHost),
) -> RunResult {
    let (ctx, host) = prepared;
    crate::test_support::run_with_host(env, prompt, args, ctx, host).await
}

/// Runs a fixture offline through the test-support driver
/// ([`crate::test_support::run_with_host`]) with a caller-customized
/// [`RunContext`] and [`RunHost`], returning the typed [`RunError`]
/// so a test can assert on its kind (limits, cancellation).
pub(super) async fn run_with_context(
    test: &TestPrompt,
    configure: impl FnOnce(RunContext, RunHost) -> (RunContext, RunHost),
) -> std::result::Result<String, RunError> {
    let env = Environment::new();
    let (mut ctx, host) = configure(test_context(EXECUTION), RunHost::new());
    ctx = ctx.vfs(TestStore::new().vfs());
    if ctx.model.is_none()
        && let Some(model) = test.models.models().first()
    {
        ctx = ctx.model(model.clone());
    }
    match crate::test_support::run_with_host(&env, &test.prompt, "", ctx, host).await {
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
