//! Shared harness for the offline execution fixtures: the correlated
//! observation [`Record`], a synchronized [`Recorder`], the offline `run`
//! helper, and the [`run_fixture`] runner that collapses the repeated parse,
//! store, and run plumbing into one call.

use std::sync::{Arc, Mutex};

use promptforge_core::execute::{ResolutionContext, RunConfig, RunError, run as run_core};
use promptforge_core::model::ModelCatalog;
use promptforge_core::observe::{Observation, Observer};
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use promptforge_core::tools::{Tool, ToolCatalog};
use promptforge_tool_picker::{Catalog, Config, ToolPicker};

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
    store: &StoreRef,
    opts: RunOptions,
) -> Result<String, RunError> {
    let picker = ToolPicker::build(Catalog::default(), Config::default())
        .expect("empty fixture picker must build");
    let models = ModelCatalog::empty();
    let tools = ToolCatalog::new(tools).expect("fixture tools are unique");
    run_core(
        prompt,
        args,
        ResolutionContext::new(&picker, &models, &tools),
        store,
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

/// The parsed prompt run plus the recorder and store an assertion needs.
pub(super) struct FixtureRun {
    pub(super) result: Result<String, RunError>,
    pub(super) recorder: Arc<Recorder>,
    pub(super) store: StoreRef,
}

/// Parses `source` and runs it offline with `args`, no tools, and either the
/// supplied `store` or a fresh in-memory one, returning the result together
/// with the recorder and store the caller asserts on.
pub(super) async fn run_fixture(
    source: &'static str,
    name: &'static str,
    execution: &'static str,
    args: &str,
    store: Option<StoreRef>,
) -> FixtureRun {
    let recorder = Arc::new(Recorder::default());
    let prompt = parse_execution_fixture(source, name, execution, recorder.as_ref());
    let store = store.unwrap_or_else(StoreRef::memory);
    let result = run(
        &prompt,
        args,
        &[],
        &store,
        RunOptions {
            execution,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
        },
    )
    .await;
    FixtureRun {
        result,
        recorder,
        store,
    }
}
