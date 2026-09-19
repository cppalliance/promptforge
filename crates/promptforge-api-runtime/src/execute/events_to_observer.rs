//! The adapter from the engine's [`Event`] values to today's host seams.
//!
//! The scheduler reports as values into the run's event buffer; the driver
//! drains that buffer after every dispatch round and hands the batch to
//! [`forward`], which replays each event onto the host's [`Observer`] and,
//! for the debug variants, its [`DebugCapture`]. Lifecycle variants become
//! the matching [`Observation`]; content variants become the `on_*` hooks;
//! `Request` and `Response` become the capture pair. The order of the
//! batch is the order the host sees, so the existing observation suites
//! hold unchanged through this path.
//!
//! The live streaming-delta callback (`on_delta`) is not an event: a delta
//! is a fragment the model client hands over mid-round, before the round's
//! reply exists to be reported, and no [`Event`] variant carries one. It
//! stays a callback on the client path and never enters the buffer.
//!
//! The `on_*` hooks take a `chain_id` and `depth` an [`Event`] does not
//! carry - its provenance names the task instead - so the adapter passes
//! zero for both. For replies, tool-call batches, and thinking that is
//! the value the scheduler already reported. For tool results and task
//! notices it is a loss: the dispatch body in `promptforge-lua` still
//! hands the calling chain's id and depth to `on_tool_result`, and the
//! notice path carried the owner's, but the emitter's `Observer` impl
//! drops both before the event is built, so a host that records them
//! (the `RuntimeEvent` log's `chain_id` and `depth` columns) now sees
//! zeros where it saw the real values. The columns are deleted with that
//! log; a host that needs the grouping reads `provenance.task` instead.

use promptforge_api_types::event::Event;

use crate::debug::{DebugCapture, DebugEvent};
use crate::observe::{Observation, Observer};

use super::event_buffer::{unit_lifecycle_variants, unit_observation};

/// The payload-carrying lifecycle and task variants
/// [`forward_lifecycle`] owns.
macro_rules! task_lifecycle_variants {
    () => {
        Event::Lua { .. }
            | Event::Other { .. }
            | Event::TaskStarted { .. }
            | Event::TaskSucceeded { .. }
            | Event::TaskFailed { .. }
            | Event::TaskCancelled { .. }
            | Event::TaskAbandoned { .. }
            | Event::TaskResumed { .. }
            | Event::TaskNote { .. }
    };
}

/// The content variants [`forward_content`] owns.
macro_rules! content_variants {
    () => {
        Event::Thinking { .. }
            | Event::AssistantReply { .. }
            | Event::AssistantToolCalls { .. }
            | Event::ToolResult { .. }
            | Event::UserInput { .. }
            | Event::TaskNotice { .. }
    };
}

/// The debug pair [`forward_debug`] owns.
macro_rules! debug_variants {
    () => {
        Event::Request { .. } | Event::Response { .. }
    };
}

/// Every variant the named group does not own: the arm each group's
/// match closes with, so it stays exhaustive without a wildcard.
macro_rules! other_groups {
    (task_lifecycle) => {
        unit_lifecycle_variants!() | content_variants!() | debug_variants!()
    };
    (content) => {
        unit_lifecycle_variants!() | task_lifecycle_variants!() | debug_variants!()
    };
    (debug) => {
        unit_lifecycle_variants!() | task_lifecycle_variants!() | content_variants!()
    };
}

/// Replays `events`, in order, onto `observer` and `debug`.
pub(crate) fn forward(
    events: Vec<Event>,
    observer: &dyn Observer,
    debug: Option<&dyn DebugCapture>,
) {
    for event in events {
        forward_one(event, observer, debug);
    }
}

/// Routes one event to the seam its group belongs to: the payload-free
/// lifecycle variants through the pair list, then the payload-carrying
/// lifecycle and task variants, the content variants, and the debug pair.
/// The match is exhaustive over [`Event`] with no wildcard, and each
/// group's own match names the other groups' variants as not its own, so
/// a new variant fails to compile here until a group claims it.
fn forward_one(event: Event, observer: &dyn Observer, debug: Option<&dyn DebugCapture>) {
    if let Some(observation) = unit_observation(&event) {
        observer.observe(event.execution(), event.section(), observation);
        return;
    }
    match event {
        // Forwarded above; the pair list and this pattern come from the
        // one declaration, so the arm is named only to keep the match
        // exhaustive.
        unit_lifecycle_variants!() => {}
        task_lifecycle_variants!() => forward_lifecycle(event, observer),
        content_variants!() => forward_content(event, observer),
        debug_variants!() => forward_debug(event, debug),
    }
}

/// The payload-carrying lifecycle and task variants, as observations.
fn forward_lifecycle(event: Event, observer: &dyn Observer) {
    match event {
        Event::Lua {
            execution,
            section,
            message,
            ..
        } => observer.observe(&execution, &section, Observation::Lua(message)),
        Event::Other {
            execution,
            section,
            message,
            ..
        } => observer.observe(&execution, &section, Observation::Other(message)),
        // Tasks.
        Event::TaskStarted {
            execution,
            section,
            task,
            target,
            origin,
            input,
            item,
            index,
            var,
            ..
        } => observer.observe(
            &execution,
            &section,
            Observation::TaskStarted {
                task,
                target,
                origin,
                input,
                item,
                index,
                var,
            },
        ),
        Event::TaskSucceeded {
            execution,
            section,
            task,
            ..
        } => observer.observe(&execution, &section, Observation::TaskSucceeded { task }),
        Event::TaskFailed {
            execution,
            section,
            task,
            ..
        } => observer.observe(&execution, &section, Observation::TaskFailed { task }),
        Event::TaskCancelled {
            execution,
            section,
            task,
            ..
        } => observer.observe(&execution, &section, Observation::TaskCancelled { task }),
        Event::TaskAbandoned {
            execution,
            section,
            task,
            reason,
            ..
        } => observer.observe(
            &execution,
            &section,
            Observation::TaskAbandoned { task, reason },
        ),
        // No observation names these yet; the escape hatch carries the
        // kind so a host's trace still shows the boundary.
        Event::TaskResumed {
            execution, section, ..
        } => observer.observe(
            &execution,
            &section,
            Observation::Other("Task resumed".to_owned()),
        ),
        Event::TaskNote {
            execution, section, ..
        } => observer.observe(
            &execution,
            &section,
            Observation::Other("Task note".to_owned()),
        ),
        // Owned by the other groups; `forward_one` routes them there.
        #[expect(
            clippy::unnested_or_patterns,
            reason = "the groups compose as or-patterns from one declaration each"
        )]
        other_groups!(task_lifecycle) => {}
    }
}

/// The content variants, as the observer's `on_*` hooks.
fn forward_content(event: Event, observer: &dyn Observer) {
    match event {
        Event::Thinking {
            execution,
            section,
            turn,
            model,
            text,
            ..
        } => observer.on_thinking(&execution, &section, 0, 0, turn, &model, &text),
        Event::AssistantReply {
            execution,
            section,
            turn,
            text,
            finish_reason,
            model,
            metrics,
            ..
        } => observer.on_assistant_reply(
            &execution,
            &section,
            0,
            0,
            turn,
            &text,
            finish_reason.as_deref(),
            &model,
            metrics.as_ref(),
        ),
        Event::AssistantToolCalls {
            execution,
            section,
            turn,
            model,
            calls,
            ..
        } => observer.on_assistant_tool_calls(&execution, &section, 0, 0, turn, &model, &calls),
        Event::ToolResult {
            execution,
            section,
            turn,
            tool_call_id,
            alias,
            content,
            trusted,
            ..
        } => observer.on_tool_result(
            &execution,
            &section,
            0,
            0,
            turn,
            &tool_call_id,
            &alias,
            &content,
            trusted,
        ),
        Event::UserInput {
            execution,
            section,
            text,
            ..
        } => observer.on_user_input(&execution, &section, &text),
        Event::TaskNotice {
            execution,
            section,
            turn,
            task,
            text,
            ..
        } => observer.on_task_notice(&execution, &section, 0, 0, turn, &task, &text),
        // Owned by the other groups; `forward_one` routes them there.
        #[expect(
            clippy::unnested_or_patterns,
            reason = "the groups compose as or-patterns from one declaration each"
        )]
        other_groups!(content) => {}
    }
}

/// The debug pair, as the capture's events; dropped when the host set no
/// capture.
fn forward_debug(event: Event, debug: Option<&dyn DebugCapture>) {
    match event {
        Event::Request {
            execution,
            section,
            turn,
            body,
            ..
        } => {
            if let Some(capture) = debug {
                capture.on_event(&execution, &section, turn, DebugEvent::Request { body });
            }
        }
        Event::Response {
            execution,
            section,
            turn,
            body,
            finish_reason,
            reasoning_content,
            ..
        } => {
            if let Some(capture) = debug {
                capture.on_event(
                    &execution,
                    &section,
                    turn,
                    DebugEvent::Response {
                        body,
                        finish_reason,
                        reasoning_content,
                    },
                );
            }
        }
        // Owned by the other groups; `forward_one` routes them there.
        #[expect(
            clippy::unnested_or_patterns,
            reason = "the groups compose as or-patterns from one declaration each"
        )]
        other_groups!(debug) => {}
    }
}

#[cfg(test)]
#[path = "events_to_observer-tests.rs"]
mod tests;
