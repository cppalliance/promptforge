//! Scheduler-side tests, split by seam into the `walk`, `live_h1`, `fanout`,
//! `concurrency`, and `failures` submodules. The model set `writer_models`,
//! its scenario `scheduler_scenario`, the concurrency ceiling `ceiling`,
//! the round and timer matchers `asks` and `is_timer`, the prompt readers
//! `request_prompts` and `issued_prompts`, the replaying `recorded`, and
//! the task-lifecycle counter `terminal_count` with its event matchers live
//! here because more than one submodule uses them.

use std::num::NonZeroUsize;

use promptforge_types::event::Event;
use promptforge_types::ids::AbandonReason;

use super::context::Recorder;
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

/// Whether `event` reports a task's start.
fn task_started(event: &Event) -> bool {
    matches!(event, Event::TaskStarted { .. })
}

/// Whether `event` reports a task's success.
fn task_succeeded(event: &Event) -> bool {
    matches!(event, Event::TaskSucceeded { .. })
}

/// Whether `event` reports a task's failure.
fn task_failed(event: &Event) -> bool {
    matches!(event, Event::TaskFailed { .. })
}

/// Whether `event` reports a task its owner cancelled.
fn task_cancelled(event: &Event) -> bool {
    matches!(event, Event::TaskCancelled { .. })
}

/// Whether `event` reports a task the run's end abandoned.
fn task_abandoned_by_run_end(event: &Event) -> bool {
    matches!(
        event,
        Event::TaskAbandoned {
            reason: AbandonReason::RunTerminated,
            ..
        }
    )
}

/// The run's events replayed onto a fresh recorder.
fn recorded(trace: &Trace) -> Recorder {
    let recorder = Recorder::default();
    trace.replay(&recorder);
    recorder
}

/// Counts the run's events `is` picks out.
fn terminal_count(trace: &Trace, is: fn(&Event) -> bool) -> usize {
    trace.events().iter().filter(|event| is(event)).count()
}

mod concurrency;
mod failures;
mod fanout;
mod live_h1;
mod walk;
