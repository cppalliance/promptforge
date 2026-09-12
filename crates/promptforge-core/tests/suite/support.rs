//! Shared harness for the offline execution fixtures: the correlated
//! observation [`Record`], a synchronized [`Recorder`], the offline `run`
//! helper, and the [`run_fixture`] runner that collapses the repeated parse,
//! store, and run plumbing into one call.

use std::sync::{Arc, Mutex};

use promptforge_core::execute::{ResolutionContext, RunConfig, RunError, run as run_core};
use promptforge_core::model::ModelCatalog;
use promptforge_core::observe::{Observation, Observer};
use promptforge_core::parser::Prompt;
use promptforge_core::store::{StoreError, StoreExt, VfsRef};
use promptforge_tool_picker::{Catalog, Config, ToolPicker};
use promptforge_tools::{Tool, ToolCatalog};
use shared_vfs::Origin;

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

/// Owned run inputs a fixture supplies: the execution id and an `Arc` observer
/// so the offline `run` helper can build a [`RunConfig`]. These fixtures never
/// reach a model, so no client or debug sink is configured.
pub(super) struct RunOptions {
    pub(super) execution: &'static str,
    pub(super) observer: Arc<dyn Observer>,
}

pub(super) async fn run(
    prompt: &Prompt,
    args: &str,
    tools: &[Arc<dyn Tool>],
    vfs: &VfsRef,
    opts: RunOptions,
) -> Result<String, RunError> {
    let picker = ToolPicker::build_with_model(
        &promptforge_tool_picker::Model::dummy(),
        Catalog::default(),
        Config::default(),
        None,
    )
    .expect("empty fixture picker must build");
    let models = ModelCatalog::empty();
    let tools = ToolCatalog::new(tools).expect("fixture tools are unique");
    run_core(
        prompt,
        args,
        ResolutionContext::new(&picker, &models, &tools),
        vfs,
        RunConfig::new(opts.execution).observer(opts.observer),
    )
    .await
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

/// Parses `source` and runs it offline with `args`, no tools, and either the
/// supplied `vfs` or a fresh stock handle, returning the result together
/// with the recorder and store the caller asserts on.
pub(super) async fn run_fixture(
    source: &'static str,
    name: &'static str,
    execution: &'static str,
    args: &str,
    vfs: Option<VfsRef>,
) -> FixtureRun {
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(source, name, execution, recorder.as_ref());
    let vfs = vfs.unwrap_or_else(promptforge_vfs::empty);
    let result = run(
        &prompt,
        args,
        &[],
        &vfs,
        RunOptions {
            execution,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
        },
    )
    .await;
    FixtureRun {
        result,
        recorder,
        store: FixtureStore(vfs),
    }
}
