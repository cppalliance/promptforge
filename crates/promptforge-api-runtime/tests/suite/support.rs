//! Shared harness for the offline execution fixtures: the correlated
//! observation [`Record`], a synchronized [`Recorder`], the offline `run`
//! helper, and the [`run_fixture`] runner that collapses the repeated parse,
//! store, and run plumbing into one call.

use std::sync::{Arc, Mutex};

use promptforge_api_runtime::parser::Prompt;
use promptforge_api_runtime::test_support::recording::{Observation, Observer};
use promptforge_api_runtime::test_support::{RunHost, TestTool, run_host};
use promptforge_api_runtime::{Environment, RunContext, RunError, RunResult};
use promptforge_api_types::timestamp::Timestamp;
use promptforge_store::{StoreError, StoreExt};
use shared_vfs::{Origin, VfsRef};

/// A [`RunContext`] for the run `name` under the fixed host inputs every
/// fixture shares: the engine takes its seed and clock from the host, and
/// no fixture here asserts on the nonce or `sys.when`.
pub(super) fn context(name: impl Into<String>) -> RunContext {
    RunContext::new(name, 1, Timestamp::UNIX_EPOCH)
}

/// One correlated observation: which execution and section emitted it, plus the
/// rendered event detail the fixtures assert on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub(super) execution: String,
    pub(super) section: String,
    pub(super) detail: String,
}

impl Record {
    /// Builds an expected record from borrowed parts, for assertions.
    pub(super) fn new(execution: &str, section: &str, detail: &str) -> Self {
        Self {
            execution: execution.to_owned(),
            section: section.to_owned(),
            detail: detail.to_owned(),
        }
    }
}

/// Owned run inputs a fixture supplies: the run name and an `Arc` observer
/// so the offline `run` helper can build the [`RunHost`] that holds the
/// observer. These fixtures never reach a model, so no client or debug
/// sink is configured.
pub(super) struct RunOptions {
    pub(super) execution: &'static str,
    pub(super) observer: Arc<dyn Observer>,
}

impl RunOptions {
    /// The host side of the fixture run: the observer alone.
    fn host(self) -> RunHost {
        RunHost::new().observer(self.observer)
    }
}

/// Prepares a fixture run against the default environment and returns the
/// prepared context, the host that holds the observer, and the run's own VFS
/// handle - the prepared router - for seeding before the run and
/// extraction after. The fixture tools are accepted for signature parity
/// only; contributing them to a run takes a capability and a declared
/// slot.
pub(super) fn prepare_run(
    prompt: &Prompt,
    tools: &[Arc<dyn TestTool>],
    opts: RunOptions,
) -> (RunContext, RunHost, VfsRef) {
    let _ = tools;
    let env = Environment::new();
    let execution = opts.execution;
    let (ctx, requirements) = env.prepare(prompt, context(execution));
    assert!(
        requirements.is_satisfied(),
        "fixture prompts declare no capabilities or model roles: {requirements:?}"
    );
    let vfs = ctx.vfs_handle().clone();
    (ctx, opts.host(), vfs)
}

/// Drives a prepared context to its result through the tokio test driver.
pub(super) async fn drive(
    prompt: &Prompt,
    args: &str,
    ctx: RunContext,
    host: RunHost,
) -> Result<String, RunError> {
    match run_host(prompt, args, ctx, host).await {
        RunResult::Ok(text) => Ok(text),
        RunResult::Cancelled => panic!("offline fixture runs are never cancelled"),
        RunResult::Failure(error) => Err(error),
    }
}

/// Prepares and runs a fixture prompt in one call.
pub(super) async fn run(
    prompt: &Prompt,
    args: &str,
    tools: &[Arc<dyn TestTool>],
    opts: RunOptions,
) -> Result<String, RunError> {
    let (ctx, host, _vfs) = prepare_run(prompt, tools, opts);
    drive(prompt, args, ctx, host).await
}

/// Runs `prompt` over a caller-built handle with no prepare pass: the raw
/// host-handle contract, for tests of custom store backends (a gated or
/// host-rooted store mount the prepare pass would replace with the run's
/// own fresh store).
pub(super) async fn run_unprepared(
    prompt: &Prompt,
    args: &str,
    vfs: VfsRef,
    opts: RunOptions,
) -> Result<String, RunError> {
    let execution = opts.execution;
    drive(prompt, args, context(execution).vfs(vfs), opts.host()).await
}

/// A synchronized observer shared by concurrent fixture runs.
#[derive(Default)]
pub(super) struct Recorder(Mutex<Vec<Record>>);

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .expect("the fixture recorder mutex must remain usable")
            .push(Record {
                execution: execution.to_owned(),
                section: section.to_owned(),
                detail: event.to_string(),
            });
    }
}

impl Recorder {
    pub(super) fn records(&self) -> Vec<Record> {
        self.0
            .lock()
            .expect("the fixture recorder mutex must remain usable")
            .clone()
    }
}

pub(super) fn parse_execution_fixture(
    source: &str,
    name: &str,
    execution: &str,
    observer: &dyn Observer,
) -> Prompt {
    // The parse-time events replay onto the recorder, as the run's will.
    let (prompt, events) = Prompt::parse(&with_test_title(source), execution);
    promptforge_api_runtime::test_support::forward(events, observer);
    prompt.unwrap_or_else(|error| panic!("fixture {name} failed to parse: {error}"))
}

/// An inline fixture that omits the required H1 title gets the shared
/// `# Test prompt` heading the in-crate harness `parse` injected before the
/// moved cases were ported; a source that already carries an H1 (every
/// fixture file, and the cases that author their own title) is parsed as
/// written.
fn with_test_title(source: &str) -> std::borrow::Cow<'_, str> {
    if source.lines().any(|line| line.starts_with("# ")) {
        std::borrow::Cow::Borrowed(source)
    } else {
        std::borrow::Cow::Owned(source.replacen("---\n\n", "---\n\n# Test prompt\n\n", 1))
    }
}

/// The run's VFS handle with per-call fresh-access store reads, for
/// post-run assertions: the run's identities dropped with it, so a fresh
/// access never meets a lingering claim.
pub(super) struct FixtureStore(VfsRef);

impl FixtureStore {
    /// Reads a store path through a fresh, immediately dropped access.
    pub(super) fn read(&self, path: &str) -> Result<String, StoreError> {
        let access = self
            .0
            .acquire(Origin::new("FixtureStore::read"))
            .map_err(StoreError::backend)?;
        self.0.store(&access).read(path)
    }
}

/// The parsed prompt run plus the recorder and store an assertion needs.
pub(super) struct FixtureRun {
    pub(super) result: Result<String, RunError>,
    pub(super) recorder: Arc<Recorder>,
    pub(super) store: FixtureStore,
}

/// Parses `source` and runs it offline with `args` and no tools, returning
/// the result together with the recorder and store the caller asserts on.
/// With `vfs` absent the run goes through prepare and the store is the
/// prepared router's handle; an explicit `vfs` is the raw host-handle
/// contract - no prepare pass, the run uses the handle as-is.
pub(super) async fn run_fixture(
    source: &str,
    name: &str,
    execution: &'static str,
    args: &str,
    vfs: Option<VfsRef>,
) -> FixtureRun {
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(source, name, execution, recorder.as_ref());
    let (result, store) = if let Some(vfs) = vfs {
        let result = run_unprepared(
            &prompt,
            args,
            vfs.clone(),
            RunOptions {
                execution,
                observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            },
        )
        .await;
        (result, vfs)
    } else {
        let (ctx, host, vfs) = prepare_run(
            &prompt,
            &[],
            RunOptions {
                execution,
                observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            },
        );
        let result = drive(&prompt, args, ctx, host).await;
        (result, vfs)
    };
    FixtureRun {
        result,
        recorder,
        store: FixtureStore(store),
    }
}
