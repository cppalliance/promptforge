//! Scheduler-side tests, split by seam into the `walk`, `live_h1`, `fanout`,
//! `store_gate`, and `failures` submodules. The context builders `writer_models`,
//! `scheduler_context`, `scheduler_context_on`, and `scheduler_context_from`, the
//! gateway-request helper `request_prompts`, and the task-lifecycle counter
//! `terminal_count` with its labels live here because more than one submodule uses
//! them.

use super::*;
use crate::model::ModelBinding;
use promptforge_model_client::model::ModelInvocation;
use promptforge_types::detail::model_id_from_validated;

/// The model set the live H1 pass would leave behind: one `writer` binding
/// as the prompt-wide default. The scheduler's tests bypass H1, so they
/// pre-fill the run's shared set directly.
pub(super) fn writer_models() -> ModelSet {
    ModelSet {
        bindings: vec![ModelBinding::new(
            "writer",
            "A general model for tests",
            model_id_from_validated("gateway", "test-model"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            NonZeroU32::new(4096).expect("4096 is non-zero"),
        )],
        default: Some("writer".to_owned()),
    }
}

/// Builds the run context and its silent host for a scheduler test: the
/// parsed prompt, an empty shared library, and the model set pre-filled.
fn scheduler_context(prompt: &Prompt) -> (RunState, RunHost) {
    scheduler_context_on(prompt, &TestStore::new(), Arc::new(NullObserver::default()))
}

/// Builds the run context and its observing host on the given store and
/// observer, so a walk test can inspect the store's contents and the
/// observation stream afterward.
pub(super) fn scheduler_context_on(
    prompt: &Prompt,
    store: &TestStore,
    observer: Arc<dyn Observer>,
) -> (RunState, RunHost) {
    scheduler_context_from(
        prompt,
        store,
        &test_context(EXECUTION),
        RunHost::new().observer(observer),
    )
}

/// Builds the run context from a finished `RunContext` and its `RunHost` on
/// the given store: the parsed prompt, an empty shared library, and the
/// model set pre-filled. Every scheduler-side context builder routes through
/// here so a test that needs an observer, limits, or both composes the
/// `RunContext` and the `RunHost` itself.
pub(super) fn scheduler_context_from(
    prompt: &Prompt,
    store: &TestStore,
    run_context: &RunContext,
    host: RunHost,
) -> (RunState, RunHost) {
    let ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &store.vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        run_context,
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = writer_models();
    (ctx, host)
}

/// The prompt in each gateway request, in arrival order.
pub(super) fn request_prompts(gateway: &ScriptedGateway) -> Vec<String> {
    gateway
        .requests()
        .iter()
        .map(|body| {
            body["messages"][0]["content"]
                .as_str()
                .expect("an infer request includes a user message")
                .to_owned()
        })
        .collect()
}

/// The stable labels of the task lifecycle observations a fanout's arms
/// report, as the recorder renders them.
const TASK_STARTED: &str = "Task started";
const TASK_SUCCEEDED: &str = "Task succeeded";
const TASK_FAILED: &str = "Task failed";
const TASK_CANCELLED: &str = "Task cancelled";
const TASK_ABANDONED_BY_RUN_END: &str = "Task abandoned: the run ended";

/// Counts one observation label in the recorder's event stream.
fn terminal_count(recorder: &Recorder, label: &str) -> usize {
    recorder
        .events()
        .iter()
        .filter(|(_, event)| event == label)
        .count()
}

mod failures;
mod fanout;
mod live_h1;
mod store_gate;
mod walk;

pub(super) use store_gate::{GateObserver, StoreGate, gated_store};
