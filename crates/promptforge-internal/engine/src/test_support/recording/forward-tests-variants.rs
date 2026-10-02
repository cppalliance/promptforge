//! The every-variant coverage test: one value of each `Event` variant,
//! forwarded alone, reaches exactly the seam the table names.

use promptforge_types::ids::{AbandonReason, TaskOrigin};

use super::*;

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
        VfsWriteSucceeded,
        VfsWriteFailed,
        VfsAppendSucceeded,
        VfsAppendFailed,
        VfsReadSucceeded,
        VfsReadFailed,
        VfsReadNumberedSucceeded,
        VfsReadNumberedFailed,
        VfsReplaceSucceeded,
        VfsReplaceFailed,
        VfsDeleteSucceeded,
        VfsDeleteFailed,
        VfsGlobSucceeded,
        VfsGlobFailed,
        VfsExistsSucceeded,
        VfsExistsFailed,
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
