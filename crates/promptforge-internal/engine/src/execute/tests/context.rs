//! Shared context builders, prompt sources, parsers, stores, task ids, and
//! recorders for the execution suites. The fixture tool-set helpers sit in
//! `tools` and are re-exported here.

use promptforge_parser::test_support::PromptSource;
use promptforge_types::ids::TaskId;

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

/// The task `id` names.
#[track_caller]
pub(super) fn task(id: &str) -> TaskId {
    id.parse().expect("a task id parses")
}

/// The `(section, detail)` sequence the run itself reported, as the suites'
/// observer sees it: the parse events ahead of it are left out.
pub(super) fn sequence(trace: &Trace) -> Vec<(String, String)> {
    trace
        .run_records()
        .into_iter()
        .map(|record| (record.section, record.detail))
        .collect()
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

/// Lua-only prompts never build the gateway client, so these run offline.
pub(super) fn parse(md: &str) -> Prompt {
    let source = if md.lines().any(|line| line.starts_with("# ")) {
        md.to_string()
    } else {
        md.replacen("---\n\n", "---\n\n# Test prompt\n\n", 1)
    };
    Prompt::parse(&source, EXECUTION).0.unwrap()
}

/// A client answering from the given scripted model, sharing its script,
/// count, and record.
pub(super) fn gateway_client(chat: &ScriptedChat) -> ScriptedChat {
    chat.clone()
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
