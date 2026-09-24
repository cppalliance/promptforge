//! Tests for the recording emitter's forwarding of event groups to their
//! seams: a batch reaches its seams in order, a debug event without a
//! capture is dropped, and every `Event` variant the suite names reaches
//! exactly one seam.

use std::sync::Mutex;

use promptforge_types::event::ReplyOrigin;
use promptforge_types::ids::{AbandonReason, ChainId, Provenance, TaskId, TaskOrigin};
use promptforge_types::metrics::ToolCallEvent;

use super::*;

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
        _metrics: Option<&promptforge_types::metrics::CallMetrics>,
        origin: ReplyOrigin,
    ) {
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{section}: reply origin={origin:?} chain={chain_id} depth={depth} turn={turn} text={text} finish={finish_reason:?} model={model}"
            ));
    }

    fn on_user_input(&self, _execution: &str, section: &str, text: &str) {
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!("{section}: input {text}"));
    }

    fn on_thinking(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        model: &str,
        text: &str,
    ) {
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{section}: thinking turn={turn} model={model} text={text}"
            ));
    }

    fn on_assistant_tool_calls(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{section}: calls turn={turn} model={model} n={}",
                calls.len()
            ));
    }

    fn on_tool_result(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        tool_call_id: &str,
        alias: &str,
        content: &str,
        trusted: bool,
    ) {
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{section}: result turn={turn} id={tool_call_id} alias={alias} content={content} trusted={trusted}"
            ));
    }

    fn on_task_notice(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        task: &TaskId,
        text: &str,
    ) {
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{section}: notice turn={turn} task={task} text={text}"
            ));
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

/// One of the three sinks an event can land in, as an index into
/// [`Recorder::counts`].
#[derive(Clone, Copy, Debug)]
enum Seam {
    Observed,
    Content,
    Captured,
}

impl Recorder {
    /// How many records each seam holds, indexed by [`Seam`].
    fn counts(&self) -> [usize; 3] {
        [
            self.observed.lock().expect("not poisoned").len(),
            self.content.lock().expect("not poisoned").len(),
            self.captured.lock().expect("not poisoned").len(),
        ]
    }
}

/// Builds one payload-free lifecycle event per named variant, each bound
/// to the observation seam.
macro_rules! unit_events {
    ($($variant:ident),* $(,)?) => {
        vec![$((
            Event::$variant {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
            },
            Seam::Observed,
        ),)*]
    };
}

/// One value of every [`Event`] variant, beside the seam that variant
/// must reach.
///
/// Hand-maintained, and deliberately written out rather than derived from
/// the forwarder's own or-patterns: `Event` is `#[non_exhaustive]` outside
/// `promptforge-types`, so no match here can be exhaustive and no
/// compiler check can hold this list to the enum. A variant added to
/// `Event` must be added here by hand.
#[expect(
    clippy::too_many_lines,
    reason = "one flat table of every event variant; splitting it would hide the coverage it exists to show"
)]
fn one_of_every_event_variant() -> Vec<(Event, Seam)> {
    let mut events = unit_events![
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
    ];
    let task: TaskId = "0.1".parse().expect("a task id parses");
    events.extend([
        (
            Event::ModelMetadataDegraded {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                message: "malformed `usage` in completion response ignored".to_owned(),
            },
            Seam::Observed,
        ),
        (
            Event::Lua {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                message: "note".to_owned(),
            },
            Seam::Observed,
        ),
        (
            Event::TaskStarted {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                task: task.clone(),
                target: "Child".to_owned(),
                origin: TaskOrigin::Author,
                input: None,
                item: None,
                index: None,
                var: serde_json::json!({}),
            },
            Seam::Observed,
        ),
        (
            Event::TaskSucceeded {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                task: task.clone(),
            },
            Seam::Observed,
        ),
        (
            Event::TaskFailed {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                task: task.clone(),
            },
            Seam::Observed,
        ),
        (
            Event::TaskCancelled {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                task: task.clone(),
            },
            Seam::Observed,
        ),
        (
            Event::TaskAbandoned {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                task: task.clone(),
                reason: AbandonReason::OwnerReturned,
            },
            Seam::Observed,
        ),
        (
            Event::TaskResumed {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                task: task.clone(),
            },
            Seam::Observed,
        ),
        (
            Event::TaskNote {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                task: task.clone(),
                text: "halfway".to_owned(),
            },
            Seam::Observed,
        ),
        (
            Event::Thinking {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                model: "m".to_owned(),
                text: "hmm".to_owned(),
            },
            Seam::Content,
        ),
        (
            Event::AssistantReply {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                text: "hi".to_owned(),
                finish_reason: Some("stop".to_owned()),
                model: "m".to_owned(),
                metrics: None,
                origin: ReplyOrigin::Chat,
            },
            Seam::Content,
        ),
        (
            Event::AssistantReply {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                text: "inferred".to_owned(),
                finish_reason: Some("stop".to_owned()),
                model: "m".to_owned(),
                metrics: None,
                origin: ReplyOrigin::Infer,
            },
            Seam::Content,
        ),
        (
            Event::AssistantToolCalls {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                model: "m".to_owned(),
                calls: vec![ToolCallEvent {
                    id: "call_1".to_owned(),
                    name: "search".to_owned(),
                    arguments: serde_json::json!({}),
                }],
            },
            Seam::Content,
        ),
        (
            Event::ToolResult {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                tool_call_id: "call_1".to_owned(),
                alias: "search".to_owned(),
                content: "found".to_owned(),
                trusted: false,
            },
            Seam::Content,
        ),
        (
            Event::UserInput {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                text: "typed".to_owned(),
            },
            Seam::Content,
        ),
        (
            Event::TaskNotice {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                task,
                text: "it ended".to_owned(),
            },
            Seam::Content,
        ),
        (
            Event::Request {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                body: serde_json::json!({}),
            },
            Seam::Captured,
        ),
        (
            Event::Response {
                execution: "run".to_owned(),
                section: "A".to_owned(),
                provenance: provenance(),
                turn: 1,
                body: serde_json::json!({}),
                finish_reason: None,
                reasoning_content: None,
            },
            Seam::Captured,
        ),
    ]);
    events
}

#[test]
fn every_event_variant_reaches_exactly_one_seam() {
    for (event, seam) in one_of_every_event_variant() {
        let named = format!("{event:?}");
        let recorder = Recorder::default();
        // A variant no group claims hits `forward_one`'s catch-all and
        // panics here; a variant a group claims but does not destructure
        // falls through that group's own `_ => {}` and records nothing.
        forward_one(event, &recorder, Some(&recorder));
        let counts = recorder.counts();
        assert_eq!(
            counts[seam as usize], 1,
            "{named} must reach the {seam:?} seam exactly once, saw {counts:?}"
        );
        assert_eq!(
            counts.iter().sum::<usize>(),
            1,
            "{named} must reach no seam but {seam:?}, saw {counts:?}"
        );
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
            origin: ReplyOrigin::Chat,
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
            "A: reply origin=Chat chain=0 depth=0 turn=1 text=hi finish=Some(\"stop\") model=m"
                .to_owned(),
            "A: input typed".to_owned(),
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
    let recorder = Recorder::default();
    forward(
        vec![Event::AssistantReply {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            turn: 1,
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
            "A: reply origin=Infer chain=0 depth=0 turn=1 text=inferred finish=Some(\"stop\") model=m"
                .to_owned(),
        ],
        "the observer must see the reply's origin"
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
    );
    assert!(recorder.observed.lock().expect("not poisoned").is_empty());
    assert!(recorder.captured.lock().expect("not poisoned").is_empty());
}
