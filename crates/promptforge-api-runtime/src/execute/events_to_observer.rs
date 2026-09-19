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
//! zero for both, the value every reply, tool-call, and thinking report
//! already carried.

use promptforge_api_types::event::Event;

use crate::debug::{DebugCapture, DebugEvent};
use crate::observe::{Observation, Observer};

use super::event_buffer::unit_observation;

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
/// Each group hands back what it does not own, so no group needs an
/// unreachable arm; whatever no group claims is a variant the adapter
/// does not know, kept visible through the escape hatch rather than lost.
fn forward_one(event: Event, observer: &dyn Observer, debug: Option<&dyn DebugCapture>) {
    if let Some(observation) = unit_observation(&event) {
        observer.observe(event.execution(), event.section(), observation);
        return;
    }
    let Some(event) = forward_lifecycle(event, observer) else {
        return;
    };
    let Some(event) = forward_content(event, observer) else {
        return;
    };
    let Some(event) = forward_debug(event, debug) else {
        return;
    };
    observer.observe(
        event.execution(),
        event.section(),
        Observation::Other(format!("{event:?}")),
    );
}

/// The payload-carrying lifecycle and task variants, as observations.
/// Returns any other event untouched.
fn forward_lifecycle(event: Event, observer: &dyn Observer) -> Option<Event> {
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
        other => return Some(other),
    }
    None
}

/// The content variants, as the observer's `on_*` hooks. Returns any
/// other event untouched.
fn forward_content(event: Event, observer: &dyn Observer) -> Option<Event> {
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
        other => return Some(other),
    }
    None
}

/// The debug pair, as the capture's events; dropped when the host set no
/// capture. Returns any other event untouched.
fn forward_debug(event: Event, debug: Option<&dyn DebugCapture>) -> Option<Event> {
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
        other => return Some(other),
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use promptforge_api_types::ids::{ChainId, Provenance, TaskId};

    use super::*;
    use crate::observe::detail;

    fn provenance() -> Provenance {
        Provenance {
            task: TaskId::from(ChainId::root()),
            seq: 0,
        }
    }

    #[derive(Default)]
    struct Recorder {
        observed: Mutex<Vec<(String, String)>>,
        content: Mutex<Vec<String>>,
        captured: Mutex<Vec<(u32, String)>>,
    }

    impl Observer for Recorder {
        fn observe(&self, _execution: &str, section: &str, event: Observation) {
            self.observed
                .lock()
                .expect("the recorder mutex is not poisoned")
                .push((section.to_owned(), event.to_string()));
        }

        fn on_assistant_reply(
            &self,
            _execution: &str,
            section: &str,
            chain_id: u32,
            depth: u32,
            turn: u32,
            text: &str,
            finish_reason: Option<&str>,
            model: &str,
            _metrics: Option<&promptforge_api_types::events::CallMetrics>,
        ) {
            self.content
                .lock()
                .expect("the recorder mutex is not poisoned")
                .push(format!(
                    "{section}: reply chain={chain_id} depth={depth} turn={turn} text={text} finish={finish_reason:?} model={model}"
                ));
        }

        fn on_user_input(&self, _execution: &str, section: &str, text: &str) {
            self.content
                .lock()
                .expect("the recorder mutex is not poisoned")
                .push(format!("{section}: input {text}"));
        }
    }

    impl DebugCapture for Recorder {
        fn on_event(&self, _execution: &str, _section: &str, turn_index: u32, event: DebugEvent) {
            let kind = match event {
                DebugEvent::Request { .. } => "request",
                DebugEvent::Response { .. } => "response",
            };
            self.captured
                .lock()
                .expect("the recorder mutex is not poisoned")
                .push((turn_index, kind.to_owned()));
        }
    }

    #[test]
    fn each_event_group_reaches_its_seam_in_batch_order() {
        let recorder = Recorder::default();
        let events = vec![
            Event::SectionStarted {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
            },
            Event::Request {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                body: serde_json::json!({}),
            },
            Event::Response {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                body: serde_json::json!({}),
                finish_reason: None,
                reasoning_content: None,
            },
            Event::AssistantReply {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                text: "hi".to_owned(),
                finish_reason: Some("stop".to_owned()),
                model: "m".to_owned(),
                metrics: None,
            },
            Event::UserInput {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                text: "typed".to_owned(),
            },
            Event::Lua {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                message: "note".to_owned(),
            },
            Event::TaskSucceeded {
                execution: "run".to_owned(),
                section: "W".to_owned(),
                provenance: provenance(),
                task: "0.1".parse().expect("a task id parses"),
            },
        ];
        forward(events, &recorder, Some(&recorder));
        assert_eq!(
            *recorder.observed.lock().expect("not poisoned"),
            vec![
                ("A".to_owned(), detail::SECTION_STARTED.to_string()),
                ("A".to_owned(), "Lua: note".to_owned()),
                ("W".to_owned(), "Task succeeded".to_owned()),
            ]
        );
        assert_eq!(
            *recorder.content.lock().expect("not poisoned"),
            vec![
                "A: reply chain=0 depth=0 turn=1 text=hi finish=Some(\"stop\") model=m".to_owned(),
                "A: input typed".to_owned(),
            ]
        );
        assert_eq!(
            *recorder.captured.lock().expect("not poisoned"),
            vec![(1, "request".to_owned()), (1, "response".to_owned())]
        );
    }

    #[test]
    fn debug_events_are_dropped_without_a_capture() {
        let recorder = Recorder::default();
        forward(
            vec![Event::Request {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                body: serde_json::json!({}),
            }],
            &recorder,
            None,
        );
        assert!(recorder.observed.lock().expect("not poisoned").is_empty());
        assert!(recorder.captured.lock().expect("not poisoned").is_empty());
    }
}
