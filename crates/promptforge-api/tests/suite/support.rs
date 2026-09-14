//! Shared harness for the offline execution fixtures: the correlated
//! observation [`Record`], a synchronized [`Recorder`], the offline `run`
//! helper, and the [`run_fixture`] runner that collapses the repeated parse,
//! store, and run plumbing into one call.

use std::sync::{Arc, Mutex};

use promptforge_api::execute::{Environment, RunContext, RunError, RunResult};
use promptforge_api::parser::Prompt;
use promptforge_store::{StoreError, StoreExt};
use promptforge_tool_picker::{Catalog, Config, ToolPicker};
use shared_promptforge_api::observe::{Observation, Observer};
use shared_promptforge_api::tools::{Tool, ToolCatalog};
use shared_vfs::{Origin, VfsRef};

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
/// so the offline `run` helper can build a [`RunContext`]. These fixtures never
/// reach a model, so no client or debug sink is configured.
pub(super) struct RunOptions {
    pub(super) execution: &'static str,
    pub(super) observer: Arc<dyn Observer>,
}

/// Prepares a fixture run against a fixture environment (dummy picker,
/// the given tools) and returns the prepared context plus the run's own
/// VFS handle - the prepared router - for seeding before the run and
/// extraction after.
pub(super) fn prepare_run(
    prompt: &Prompt,
    tools: &[Arc<dyn Tool>],
    opts: RunOptions,
) -> (RunContext, VfsRef) {
    let picker = ToolPicker::build_with_model(
        &promptforge_tool_picker::Model::dummy(),
        Catalog::default(),
        Config::default(),
        None,
    )
    .expect("empty fixture picker must build");
    let tools = ToolCatalog::new(tools).expect("fixture tools are unique");
    let env = Environment::new().picker(picker).tools(tools);
    let ctx = RunContext::new(opts.execution).observer(opts.observer);
    let (ctx, requirements) = env.prepare(prompt, ctx);
    assert!(
        requirements.is_satisfied(),
        "fixture prompts declare no capabilities or model roles: {requirements:?}"
    );
    let vfs = ctx.vfs_handle().clone();
    (ctx, vfs)
}

/// Drives a prepared context to its result through the free `run`.
pub(super) async fn drive(
    prompt: &Prompt,
    args: &str,
    ctx: RunContext,
) -> Result<String, RunError> {
    match promptforge_api::execute::run(prompt, args, ctx).await {
        RunResult::Ok(text) => Ok(text),
        RunResult::Cancelled => panic!("offline fixture runs are never cancelled"),
        RunResult::Failure(error) => Err(error),
    }
}

/// Prepares and runs a fixture prompt in one call.
pub(super) async fn run(
    prompt: &Prompt,
    args: &str,
    tools: &[Arc<dyn Tool>],
    opts: RunOptions,
) -> Result<String, RunError> {
    let (ctx, _vfs) = prepare_run(prompt, tools, opts);
    drive(prompt, args, ctx).await
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
    let ctx = RunContext::new(opts.execution)
        .observer(opts.observer)
        .vfs(vfs);
    drive(prompt, args, ctx).await
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
    Prompt::parse(source, execution, observer)
        .unwrap_or_else(|error| panic!("fixture {name} failed to parse: {error}"))
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
    source: &'static str,
    name: &'static str,
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
        let (ctx, vfs) = prepare_run(
            &prompt,
            &[],
            RunOptions {
                execution,
                observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            },
        );
        let result = drive(&prompt, args, ctx).await;
        (result, vfs)
    };
    FixtureRun {
        result,
        recorder,
        store: FixtureStore(store),
    }
}
