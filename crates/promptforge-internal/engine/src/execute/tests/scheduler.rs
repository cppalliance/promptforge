//! Scheduler-side tests, split by seam into the `walk`, `live_h1`, `fanout`,
//! `concurrency`, and `failures` submodules. The model set `writer_models`,
//! its scenario `scheduler_scenario`, the concurrency ceiling `ceiling`,
//! the round and timer matchers `asks` and `is_timer`, the prompt readers
//! `request_prompts` and `issued_prompts`, the replaying `recorded`, and
//! the task-lifecycle counter `terminal_count` with its labels live here
//! because more than one submodule uses them.

use std::num::NonZeroUsize;

use super::*;
use crate::error::DepthVia;
use crate::execute::EffectRecord;
use crate::test_support::rig::RigTrace;
use crate::test_support::scenario::{infer_prompt, test_binding};

/// The model set the live H1 pass would leave behind: one `writer` binding
/// as the prompt-wide default. The scheduler's tests bypass H1, so they
/// pre-fill the run's shared set directly.
fn writer_models() -> ModelSet {
    ModelSet::from_parts(
        vec![test_binding(
            "writer",
            "A general model for tests",
            "test-model",
        )],
        Some("writer".to_owned()),
    )
}

/// A run of `source` over the [`writer_models`], unprepared, so the run
/// starts with that set and no catalog.
pub(super) fn scheduler_scenario(source: impl Into<String>) -> Scenario {
    Scenario::new(source).unprepared().models(writer_models())
}

/// The run's concurrency ceiling narrowed to `n` admitted tasks.
#[track_caller]
pub(super) fn ceiling(n: usize) -> RunLimits {
    RunLimits::new().max_concurrency(NonZeroUsize::new(n).expect("the ceiling is non-zero"))
}

/// Whether an effect is the model round a `models.infer(prompt)` sends,
/// for a scenario to hold and a test to release.
pub(super) fn asks(prompt: &str) -> impl Fn(&Effect) -> bool + 'static {
    let prompt = prompt.to_owned();
    move |effect| matches!(effect, Effect::Chat { .. }) && infer_prompt(effect) == prompt
}

/// Whether an effect is a timer, for a scenario to hold when the member
/// it races must win, and a test to release.
pub(super) fn is_timer(effect: &Effect) -> bool {
    matches!(effect, Effect::Timer { .. })
}

/// The prompt in each gateway request, in arrival order.
pub(super) fn request_prompts(gateway: &ScriptedChat) -> Vec<String> {
    gateway
        .requests()
        .iter()
        .map(|body| body.messages[0].content().to_owned())
        .collect()
}

/// The prompt of each model round the run issued, in issue order, held
/// rounds included.
pub(super) fn issued_prompts(rig: &RigTrace) -> Vec<String> {
    rig.issued()
        .iter()
        .filter_map(|record| match record {
            EffectRecord::Chat { messages, .. } => {
                messages[0]["content"].as_str().map(str::to_owned)
            }
            _ => None,
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

/// The run's events replayed onto a fresh recorder.
fn recorded(trace: &Trace) -> Recorder {
    let recorder = Recorder::default();
    trace.replay(&recorder);
    recorder
}

/// Counts one observation label in the recorder's event stream.
fn terminal_count(recorder: &Recorder, label: &str) -> usize {
    recorder
        .events()
        .iter()
        .filter(|(_, event)| event == label)
        .count()
}

mod concurrency;
mod failures;
mod fanout;
mod live_h1;
mod walk;
