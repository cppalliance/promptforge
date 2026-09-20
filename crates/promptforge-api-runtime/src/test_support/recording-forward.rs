//! The adapter from the engine's [`Event`] values to the suites' recording
//! seams: each event replayed onto the [`Observer`] and, for the debug
//! variants, the [`DebugCapture`].
//!
//! Lifecycle variants become the matching [`Observation`]; content
//! variants become the `on_*` hooks; `Request` and `Response` become the
//! capture pair. The order of the batch is the order the observer sees.
//!
//! The `on_*` hooks take a `chain_id` and `depth` an [`Event`] does not
//! carry - its provenance names the task instead - so the adapter passes
//! zero for both; a suite that needs the grouping reads `provenance.task`
//! off the events themselves.

use promptforge_api_types::event::Event;

use super::{DebugCapture, DebugEvent, Observation, Observer};

/// Declares the payload-free lifecycle pairs once and derives the
/// event-to-observation fold from the one list.
macro_rules! lifecycle_pairs {
    ($($variant:ident),* $(,)?) => {
        /// The payload-free [`Observation`] matching a payload-free
        /// [`Event`], or `None` for any other variant.
        fn unit_observation(event: &Event) -> Option<Observation> {
            Some(match event {
                $(Event::$variant { .. } => Observation::$variant,)*
                _ => return None,
            })
        }
    };
}

lifecycle_pairs! {
    ParseStarted,
    ParseSucceeded,
    ParseFailed,
    RunStarted,
    RunSucceeded,
    RunFailed,
    SectionStarted,
    SectionFinished,
    ModelTurnCompleted,
    ModelTurnFailed,
    ModelTurnTruncated,
    ToolCallSucceeded,
    ToolCallFailed,
    LuaCompilationStarted,
    LuaCompilationSucceeded,
    LuaCompilationFailed,
    LuaSharedLoadStarted,
    LuaSharedLoadSucceeded,
    LuaSharedLoadFailed,
    LuaChunkStarted,
    LuaChunkSucceeded,
    LuaChunkFailed,
    LuaReplyBindingStarted,
    LuaReplyBindingSucceeded,
    LuaReplyBindingFailed,
    LuaTeardownStarted,
    LuaTeardownSucceeded,
    ToolScopeValidationStarted,
    ToolScopeValidationSucceeded,
    ToolScopeValidationFailed,
    ModelCatalogValidationStarted,
    ModelCatalogValidationSucceeded,
    ModelCatalogValidationFailed,
    StoreWriteSucceeded,
    StoreWriteFailed,
    StoreAppendSucceeded,
    StoreAppendFailed,
    StoreReadSucceeded,
    StoreReadFailed,
    StoreReadNumberedSucceeded,
    StoreReadNumberedFailed,
    StoreReplaceSucceeded,
    StoreReplaceFailed,
    StoreDeleteSucceeded,
    StoreDeleteFailed,
    StoreGlobSucceeded,
    StoreGlobFailed,
    UserInputWaitStarted,
}

/// The payload-carrying lifecycle and task variants
/// [`forward_lifecycle`] owns.
macro_rules! task_lifecycle_variants {
    () => {
        Event::Lua { .. }
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

/// Replays `events`, in order, onto `observer` and `debug`.
pub fn forward(events: Vec<Event>, observer: &dyn Observer, debug: Option<&dyn DebugCapture>) {
    for event in events {
        forward_one(event, observer, debug);
    }
}

/// Routes one event to the seam its group belongs to. [`Event`] is
/// `#[non_exhaustive]` in `promptforge-api-types`, so the match cannot be
/// total here and no compiler check holds the groups to the enum. What
/// holds instead is
/// `tests::every_event_variant_reaches_exactly_one_seam`, which drives one
/// value of every variant through this function and asserts each reaches
/// exactly one seam: it covers the variants written into its own list, so
/// a variant added to `Event` is covered only once someone adds it to a
/// group's or-pattern and to that list, both by hand.
///
/// Within that list the routing is total: a variant no group claims panics
/// naming itself rather than passing a suite vacuously, and a variant a
/// group claims but does not destructure records nothing and fails the
/// test.
///
/// # Panics
///
/// When `event` is a variant none of the groups above claims.
pub fn forward_one(event: Event, observer: &dyn Observer, debug: Option<&dyn DebugCapture>) {
    if let Some(observation) = unit_observation(&event) {
        observer.observe(event.execution(), event.section(), observation);
        return;
    }
    match event {
        task_lifecycle_variants!() => forward_lifecycle(event, observer),
        content_variants!() => forward_content(event, observer),
        debug_variants!() => forward_debug(event, debug),
        // The payload-free variants were forwarded above; anything else
        // is a variant no group claims (`Event` is `#[non_exhaustive]` in
        // `promptforge-api-types`), which the test recorder must never
        // drop in silence.
        _ => unreachable!("Event variant no group claims: {event:?}"),
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
        // Routed here by `forward_one` for this group alone; `Event` is
        // `#[non_exhaustive]` in `promptforge-api-types`.
        _ => {}
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
        // Routed here by `forward_one` for this group alone; `Event` is
        // `#[non_exhaustive]` in `promptforge-api-types`.
        _ => {}
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
        // Routed here by `forward_one` for this group alone; `Event` is
        // `#[non_exhaustive]` in `promptforge-api-types`.
        _ => {}
    }
}

#[cfg(test)]
#[path = "recording-forward-tests.rs"]
mod tests;
