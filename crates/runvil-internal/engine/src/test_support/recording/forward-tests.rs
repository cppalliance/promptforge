//! Tests for the recording emitter's forwarding of event groups to their
//! seams: a batch reaches its seams in order, a tool result keeps its
//! coordinates and trust, a library load keeps its library, a debug event
//! without a capture is dropped, and every `Event` variant the suite names
//! reaches exactly one seam.

use std::sync::Mutex;

use runvil_types::event::ReplyOrigin;
use runvil_types::ids::{ChainId, Provenance, RoundId, TaskId};
use runvil_types::metrics::ToolCallEvent;

use super::*;

#[path = "forward-tests-variants.rs"]
mod variants;

fn provenance() -> Provenance {
    Provenance::new(TaskId::from(ChainId::root()), 0)
}

#[derive(Default)]
struct SeamRecorder {
    observed: Mutex<Vec<(String, String)>>,
    content: Mutex<Vec<String>>,
    captured: Mutex<Vec<(u32, String)>>,
}

impl Observer for SeamRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.observed
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push((section.to_owned(), event.to_string()));
    }

    fn on_assistant_reply(&self, report: ReplyReport<'_>) {
        let ReplyReport {
            execution,
            section,
            turn,
            text,
            finish_reason,
            model,
            origin,
            ..
        } = report;
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{execution}/{section}: reply origin={origin:?} turn={turn} text={text} finish={finish_reason:?} model={model}"
            ));
    }

    fn on_thinking(&self, report: ThinkingReport<'_>) {
        let ThinkingReport {
            execution,
            section,
            turn,
            model,
            text,
        } = report;
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{execution}/{section}: thinking turn={turn} model={model} text={text}"
            ));
    }

    fn on_assistant_tool_calls(&self, report: ToolCallsReport<'_>) {
        let ToolCallsReport {
            execution,
            section,
            turn,
            model,
            calls,
        } = report;
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{execution}/{section}: calls turn={turn} model={model} n={}",
                calls.len()
            ));
    }

    fn on_tool_result(&self, report: ToolResultReport<'_>) {
        let ToolResultReport {
            execution,
            section,
            turn,
            tool_call_id,
            alias,
            content,
            trust,
        } = report;
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{execution}/{section}: result turn={turn} id={tool_call_id} alias={alias} content={content} trust={trust:?}"
            ));
    }

    fn on_task_notice(&self, report: TaskNoticeReport<'_>) {
        let TaskNoticeReport {
            execution,
            section,
            turn,
            task,
            text,
        } = report;
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{execution}/{section}: notice turn={turn} task={task} text={text}"
            ));
    }
}

impl DebugCapture for SeamRecorder {
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
    let recorder = SeamRecorder::default();
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
            round: RoundId::new(0),
            text: "hi".to_owned(),
            finish_reason: Some("stop".to_owned()),
            model: "m".to_owned(),
            metrics: None,
            origin: ReplyOrigin::Chat,
        },
        Event::Thinking {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            turn: 1,
            round: RoundId::new(0),
            model: "m".to_owned(),
            text: "hmm".to_owned(),
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
    forward_impl(events, &recorder, Some(&recorder));
    assert_eq!(
        *recorder.observed.lock().expect("not poisoned"),
        vec![
            ("A".to_owned(), Observation::SectionStarted.to_string()),
            ("A".to_owned(), "Lua: note".to_owned()),
            ("W".to_owned(), "Task succeeded".to_owned()),
        ]
    );
    assert_eq!(
        *recorder.content.lock().expect("not poisoned"),
        vec![
            "run/A: reply origin=Chat turn=1 text=hi finish=Some(\"stop\") model=m".to_owned(),
            "run/A: thinking turn=1 model=m text=hmm".to_owned(),
        ]
    );
    assert_eq!(
        *recorder.captured.lock().expect("not poisoned"),
        vec![(1, "request".to_owned()), (1, "response".to_owned())]
    );
}

#[test]
fn a_reply_forwards_its_origin_to_the_observer() {
    // A reply's provenance reaches the observer: dropping it, defaulting it
    // to `Chat`, or routing it to a second kind would change this line.
    let recorder = SeamRecorder::default();
    forward(
        vec![Event::AssistantReply {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            turn: 1,
            round: RoundId::new(0),
            text: "inferred".to_owned(),
            finish_reason: Some("stop".to_owned()),
            model: "m".to_owned(),
            metrics: None,
            origin: ReplyOrigin::Infer,
        }],
        &recorder,
    );
    assert_eq!(
        *recorder.content.lock().expect("not poisoned"),
        vec![
            "run/A: reply origin=Infer turn=1 text=inferred finish=Some(\"stop\") model=m"
                .to_owned(),
        ],
        "the observer must see the reply's origin"
    );
}

#[test]
fn a_tool_result_forwards_its_coordinates_and_trust_to_the_observer() {
    // Each report names its own event's run and section and its trust
    // level: swapping the coordinates, dropping one, or inverting the
    // trust mapping would change these lines.
    let recorder = SeamRecorder::default();
    let result = |execution: &str, trusted: bool| Event::ToolResult {
        execution: execution.to_owned(),
        section: "A".to_owned(),
        provenance: provenance(),
        turn: 2,
        tool_call_id: "call_1".to_owned(),
        alias: "search".to_owned(),
        tool: None,
        content: "found".to_owned(),
        trusted,
    };
    forward(
        vec![result("run-1", true), result("run-2", false)],
        &recorder,
    );
    assert_eq!(
        *recorder.content.lock().expect("not poisoned"),
        vec![
            "run-1/A: result turn=2 id=call_1 alias=search content=found trust=Trusted".to_owned(),
            "run-2/A: result turn=2 id=call_1 alias=search content=found trust=Untrusted"
                .to_owned(),
        ],
        "the observer must see each result's own run and trust level"
    );
}

#[test]
fn a_library_load_forwards_its_library_to_the_observer() {
    let recorder = SeamRecorder::default();
    forward(
        vec![
            Event::LuaLibraryLoadStarted {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                library: "## shared_stuff".to_owned(),
            },
            Event::LuaLibraryLoadSucceeded {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                library: "## shared_stuff".to_owned(),
            },
            Event::LuaLibraryLoadFailed {
                execution: "run".to_owned(),
                section: "B".to_owned(),
                provenance: provenance(),
                library: "## helpers".to_owned(),
            },
        ],
        &recorder,
    );
    assert_eq!(
        *recorder.observed.lock().expect("not poisoned"),
        vec![
            (
                "A".to_owned(),
                "Lua library load started: ## shared_stuff".to_owned()
            ),
            (
                "A".to_owned(),
                "Lua library load succeeded: ## shared_stuff".to_owned()
            ),
            (
                "B".to_owned(),
                "Lua library load failed: ## helpers".to_owned()
            ),
        ],
        "each phase reaches its own observation with the library it named"
    );
}

#[test]
fn debug_events_are_dropped_without_a_capture() {
    let recorder = SeamRecorder::default();
    forward(
        vec![Event::Request {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            turn: 1,
            body: serde_json::json!({}),
        }],
        &recorder,
    );
    assert!(recorder.observed.lock().expect("not poisoned").is_empty());
    assert!(recorder.captured.lock().expect("not poisoned").is_empty());
}
