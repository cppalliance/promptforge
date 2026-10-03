//! Tests for `Event` serde round trips and coordinate exposure.

use serde_json::json;

use super::Event;
use super::ReplyOrigin;
use super::lifecycle::{self, Lifecycle};
use crate::ids::{AbandonReason, Provenance, RoundId, TaskId, TaskOrigin};
use crate::metrics::{CallMetrics, ToolCallEvent, Usage};

fn task(path: &str) -> TaskId {
    path.parse().expect("a task id parses")
}

fn provenance(path: &str, seq: u32) -> Provenance {
    Provenance {
        task: task(path),
        seq,
    }
}

fn sample_metrics() -> CallMetrics {
    CallMetrics {
        usage: Some(Usage {
            prompt_tokens: 7,
            completion_tokens: 3,
            total_tokens: 10,
            cached_tokens: None,
            reasoning_tokens: None,
        }),
        llama: None,
        vllm: None,
        client: None,
    }
}

fn round_trips(event: &Event) {
    let line = serde_json::to_string(event).expect("every event serializes");
    assert!(
        !line.contains('\n'),
        "one event must serialize to one line: {line}"
    );
    let back: Event = serde_json::from_str(&line).expect("an event's own output deserializes");
    assert_eq!(&back, event, "{line}");
}

/// The lifecycle constant that builds `event`, or `None` for a variant
/// with a payload. There is no wildcard arm, so a new variant fails to
/// compile here until it is mapped; a new payload variant also needs a
/// sample in `payload_samples`.
fn lifecycle_of(event: &Event) -> Option<Lifecycle> {
    match event {
        Event::ParseStarted { .. } => Some(lifecycle::PARSE_STARTED),
        Event::ParseSucceeded { .. } => Some(lifecycle::PARSE_SUCCEEDED),
        Event::ParseFailed { .. } => Some(lifecycle::PARSE_FAILED),
        Event::RunStarted { .. } => Some(lifecycle::RUN_STARTED),
        Event::RunSucceeded { .. } => Some(lifecycle::RUN_SUCCEEDED),
        Event::RunFailed { .. } => Some(lifecycle::RUN_FAILED),
        Event::SectionStarted { .. } => Some(lifecycle::SECTION_STARTED),
        Event::SectionFinished { .. } => Some(lifecycle::SECTION_FINISHED),
        Event::ModelTurnCompleted { .. } => Some(lifecycle::MODEL_TURN_COMPLETED),
        Event::ModelTurnFailed { .. } => Some(lifecycle::MODEL_TURN_FAILED),
        Event::ModelTurnTruncated { .. } => Some(lifecycle::MODEL_TURN_TRUNCATED),
        Event::ToolCallSucceeded { .. } => Some(lifecycle::TOOL_CALL_SUCCEEDED),
        Event::ToolCallFailed { .. } => Some(lifecycle::TOOL_CALL_FAILED),
        Event::LuaCompilationStarted { .. } => Some(lifecycle::LUA_COMPILATION_STARTED),
        Event::LuaCompilationSucceeded { .. } => Some(lifecycle::LUA_COMPILATION_SUCCEEDED),
        Event::LuaCompilationFailed { .. } => Some(lifecycle::LUA_COMPILATION_FAILED),
        Event::LuaSharedLoadStarted { .. } => Some(lifecycle::LUA_SHARED_LOAD_STARTED),
        Event::LuaSharedLoadSucceeded { .. } => Some(lifecycle::LUA_SHARED_LOAD_SUCCEEDED),
        Event::LuaSharedLoadFailed { .. } => Some(lifecycle::LUA_SHARED_LOAD_FAILED),
        Event::LuaChunkStarted { .. } => Some(lifecycle::LUA_CHUNK_STARTED),
        Event::LuaChunkSucceeded { .. } => Some(lifecycle::LUA_CHUNK_SUCCEEDED),
        Event::LuaChunkFailed { .. } => Some(lifecycle::LUA_CHUNK_FAILED),
        Event::LuaReplyBindingStarted { .. } => Some(lifecycle::LUA_REPLY_BINDING_STARTED),
        Event::LuaReplyBindingSucceeded { .. } => Some(lifecycle::LUA_REPLY_BINDING_SUCCEEDED),
        Event::LuaReplyBindingFailed { .. } => Some(lifecycle::LUA_REPLY_BINDING_FAILED),
        Event::LuaTeardownStarted { .. } => Some(lifecycle::LUA_TEARDOWN_STARTED),
        Event::LuaTeardownSucceeded { .. } => Some(lifecycle::LUA_TEARDOWN_SUCCEEDED),
        Event::ToolScopeValidationStarted { .. } => Some(lifecycle::TOOL_SCOPE_VALIDATION_STARTED),
        Event::ToolScopeValidationSucceeded { .. } => {
            Some(lifecycle::TOOL_SCOPE_VALIDATION_SUCCEEDED)
        }
        Event::ToolScopeValidationFailed { .. } => Some(lifecycle::TOOL_SCOPE_VALIDATION_FAILED),
        Event::ModelCatalogValidationStarted { .. } => {
            Some(lifecycle::MODEL_CATALOG_VALIDATION_STARTED)
        }
        Event::ModelCatalogValidationSucceeded { .. } => {
            Some(lifecycle::MODEL_CATALOG_VALIDATION_SUCCEEDED)
        }
        Event::ModelCatalogValidationFailed { .. } => {
            Some(lifecycle::MODEL_CATALOG_VALIDATION_FAILED)
        }
        Event::VfsWriteSucceeded { .. } => Some(lifecycle::VFS_WRITE_SUCCEEDED),
        Event::VfsWriteFailed { .. } => Some(lifecycle::VFS_WRITE_FAILED),
        Event::VfsAppendSucceeded { .. } => Some(lifecycle::VFS_APPEND_SUCCEEDED),
        Event::VfsAppendFailed { .. } => Some(lifecycle::VFS_APPEND_FAILED),
        Event::VfsReadSucceeded { .. } => Some(lifecycle::VFS_READ_SUCCEEDED),
        Event::VfsReadFailed { .. } => Some(lifecycle::VFS_READ_FAILED),
        Event::VfsReadNumberedSucceeded { .. } => Some(lifecycle::VFS_READ_NUMBERED_SUCCEEDED),
        Event::VfsReadNumberedFailed { .. } => Some(lifecycle::VFS_READ_NUMBERED_FAILED),
        Event::VfsReplaceSucceeded { .. } => Some(lifecycle::VFS_REPLACE_SUCCEEDED),
        Event::VfsReplaceFailed { .. } => Some(lifecycle::VFS_REPLACE_FAILED),
        Event::VfsDeleteSucceeded { .. } => Some(lifecycle::VFS_DELETE_SUCCEEDED),
        Event::VfsDeleteFailed { .. } => Some(lifecycle::VFS_DELETE_FAILED),
        Event::VfsGlobSucceeded { .. } => Some(lifecycle::VFS_GLOB_SUCCEEDED),
        Event::VfsGlobFailed { .. } => Some(lifecycle::VFS_GLOB_FAILED),
        Event::VfsExistsSucceeded { .. } => Some(lifecycle::VFS_EXISTS_SUCCEEDED),
        Event::VfsExistsFailed { .. } => Some(lifecycle::VFS_EXISTS_FAILED),
        Event::ModelMetadataDegraded { .. }
        | Event::Lua { .. }
        | Event::TaskStarted { .. }
        | Event::TaskSucceeded { .. }
        | Event::TaskFailed { .. }
        | Event::TaskCancelled { .. }
        | Event::TaskAbandoned { .. }
        | Event::TaskResumed { .. }
        | Event::Thinking { .. }
        | Event::AssistantReply { .. }
        | Event::AssistantToolCalls { .. }
        | Event::ToolResult { .. }
        | Event::TaskNotice { .. }
        | Event::TaskNote { .. }
        | Event::Request { .. }
        | Event::Response { .. } => None,
    }
}

/// One sample of every variant with a payload; the payload-free variants
/// come from `lifecycle::ALL`.
fn payload_samples() -> Vec<Event> {
    let mut samples = lifecycle_payload_samples();
    samples.extend(task_samples());
    samples.extend(content_samples());
    samples.extend(debug_samples());
    samples
}

fn lifecycle_payload_samples() -> Vec<Event> {
    vec![
        Event::Lua {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 1),
            message: "checkpoint".to_owned(),
        },
        Event::ModelMetadataDegraded {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 8),
            turn: 2,
            message: "malformed `usage` in completion response ignored: invalid type: string \"lots\", expected u64".to_owned(),
        },
    ]
}

fn task_samples() -> Vec<Event> {
    vec![
        Event::TaskStarted {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 2),
            task: task("0.0"),
            target: "Worker".to_owned(),
            origin: TaskOrigin::Author,
            input: Some("arg text".to_owned()),
            item: Some(json!({ "key": "value" })),
            index: Some(3),
            var: json!({ "topic": "leap days" }),
        },
        Event::TaskSucceeded {
            execution: "run-1".to_owned(),
            section: "Worker".to_owned(),
            provenance: provenance("0.0", 1),
            task: task("0.0"),
        },
        Event::TaskFailed {
            execution: "run-1".to_owned(),
            section: "Worker".to_owned(),
            provenance: provenance("0.1", 1),
            task: task("0.1"),
        },
        Event::TaskCancelled {
            execution: "run-1".to_owned(),
            section: "Worker".to_owned(),
            provenance: provenance("0.2", 1),
            task: task("0.2"),
        },
        Event::TaskAbandoned {
            execution: "run-1".to_owned(),
            section: "Worker".to_owned(),
            provenance: provenance("0.0", 4),
            task: task("0.0"),
            reason: AbandonReason::ToolLoopExhausted,
        },
        Event::TaskResumed {
            execution: "run-1".to_owned(),
            section: "Worker".to_owned(),
            provenance: provenance("0.3", 0),
            task: task("0.3"),
        },
    ]
}

fn content_samples() -> Vec<Event> {
    let reply = |seq, origin| Event::AssistantReply {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", seq),
        turn: 2,
        round: RoundId::new(1),
        text: "hello".to_owned(),
        finish_reason: Some("stop".to_owned()),
        model: "llama-3".to_owned(),
        metrics: Some(sample_metrics()),
        origin,
    };
    vec![
        Event::Thinking {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 14),
            turn: 2,
            round: RoundId::new(1),
            model: "llama-3".to_owned(),
            text: "considering".to_owned(),
        },
        reply(3, ReplyOrigin::Chat),
        reply(7, ReplyOrigin::Infer),
        Event::AssistantToolCalls {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 4),
            turn: 2,
            round: RoundId::new(1),
            model: "llama-3".to_owned(),
            calls: vec![ToolCallEvent {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                arguments: json!({ "path": "notes.txt" }),
            }],
        },
        Event::ToolResult {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 5),
            turn: 2,
            tool_call_id: "call_1".to_owned(),
            alias: "read_file".to_owned(),
            content: "file contents".to_owned(),
            trusted: false,
        },
        Event::TaskNotice {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 15),
            turn: 3,
            task: task("0.1"),
            text: "Task id=0.1 (## Worker) completed: done".to_owned(),
        },
        Event::TaskNote {
            execution: "run-1".to_owned(),
            section: "Worker".to_owned(),
            provenance: provenance("0.1", 2),
            task: task("0.1"),
            text: "halfway".to_owned(),
        },
    ]
}

fn debug_samples() -> Vec<Event> {
    vec![
        Event::Request {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 16),
            turn: 2,
            body: json!({ "messages": [] }),
        },
        Event::Response {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 6),
            turn: 2,
            body: json!({ "choices": [] }),
            finish_reason: Some("length".to_owned()),
            reasoning_content: None,
        },
    ]
}

#[test]
fn every_variant_round_trips_through_serde() {
    for (_, build) in lifecycle::ALL {
        round_trips(&build(
            "run-1".to_owned(),
            "Gather".to_owned(),
            provenance("0", 0),
        ));
    }
    for event in payload_samples() {
        round_trips(&event);
    }
}

#[test]
fn every_payload_free_variant_maps_to_its_lifecycle_constant() {
    let coordinates = || ("run-1".to_owned(), "Gather".to_owned(), provenance("0", 0));
    for (variant, build) in lifecycle::ALL {
        let (execution, section, provenance) = coordinates();
        let event = build(execution, section, provenance);
        let mapped = lifecycle_of(&event)
            .unwrap_or_else(|| panic!("{variant} is mapped to no lifecycle constant"));
        let (execution, section, provenance) = coordinates();
        assert_eq!(
            mapped(execution, section, provenance),
            event,
            "{variant} is mapped to another variant's constant"
        );
    }
    for event in payload_samples() {
        assert!(
            lifecycle_of(&event).is_none(),
            "a payload variant is mapped to a lifecycle constant: {event:?}"
        );
    }
}

#[test]
fn a_reply_origin_defaults_to_chat() {
    assert_eq!(ReplyOrigin::default(), ReplyOrigin::Chat);
}

#[test]
fn a_reply_serializes_its_origin_and_its_round() {
    let event = Event::AssistantReply {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 20),
        turn: 1,
        round: RoundId::new(4),
        text: "inferred".to_owned(),
        finish_reason: None,
        model: "llama-3".to_owned(),
        metrics: None,
        origin: ReplyOrigin::Infer,
    };
    let line = serde_json::to_string(&event).expect("an event serializes");
    assert!(
        line.contains(r#""origin":"infer""#),
        "the origin must reach the wire: {line}"
    );
    assert!(
        line.contains(r#""round":4"#),
        "the round must reach the wire as a bare number: {line}"
    );
}

#[test]
fn a_reply_without_origin_reads_back_as_chat() {
    // A line that leaves `origin` out still parses, defaulting to a chat
    // reply. Without `#[serde(default)]` this deserialization fails.
    let line = r#"{"kind":"assistant_reply","execution":"run-1","section":"Gather","provenance":{"task":"0","seq":1},"turn":1,"round":0,"text":"hi","finish_reason":null,"model":"llama-3","metrics":null}"#;
    let event: Event = serde_json::from_str(line).expect("an old reply parses");
    match event {
        Event::AssistantReply { origin, .. } => assert_eq!(origin, ReplyOrigin::Chat),
        other => panic!("expected an assistant reply, got {other:?}"),
    }
}

#[test]
fn a_serialized_event_is_tagged_by_kind_with_its_coordinates_beside_the_payload() {
    // The tag and the three coordinates are the log schema the Harness
    // writes `task_id` and `task_seq` from without inspecting the payload;
    // renaming any of them breaks every log written before it.
    let event = Event::VfsWriteSucceeded {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0.2", 9),
    };
    assert_eq!(
        serde_json::to_string(&event).expect("an event serializes"),
        r#"{"kind":"vfs_write_succeeded","execution":"run-1","section":"Gather","provenance":{"task":"0.2","seq":9}}"#
    );
    let notice = Event::TaskNotice {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 10),
        turn: 3,
        task: task("0.1"),
        text: "Task id=0.1 (## Worker) completed: done".to_owned(),
    };
    assert_eq!(
        serde_json::to_string(&notice).expect("an event serializes"),
        r#"{"kind":"task_notice","execution":"run-1","section":"Gather","provenance":{"task":"0","seq":10},"turn":3,"task":"0.1","text":"Task id=0.1 (## Worker) completed: done"}"#
    );
}

#[test]
fn a_degraded_metadata_report_serializes_its_turn_and_message_under_its_kind() {
    let event = Event::ModelMetadataDegraded {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 3),
        turn: 1,
        message: "completion response named no string `model`; recorded as empty".to_owned(),
    };
    assert_eq!(
        serde_json::to_string(&event).expect("an event serializes"),
        r#"{"kind":"model_metadata_degraded","execution":"run-1","section":"Gather","provenance":{"task":"0","seq":3},"turn":1,"message":"completion response named no string `model`; recorded as empty"}"#
    );
}

#[test]
fn every_event_exposes_its_coordinates() {
    let events = [
        Event::SectionFinished {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 11),
        },
        Event::Lua {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 12),
            message: "hello".to_owned(),
        },
        Event::TaskResumed {
            execution: "run-1".to_owned(),
            section: "Worker".to_owned(),
            provenance: provenance("0.1", 0),
            task: task("0.1"),
        },
        Event::Request {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 13),
            turn: 4,
            body: json!({ "messages": [] }),
        },
    ];
    for event in &events {
        assert_eq!(event.execution(), "run-1");
    }
    assert_eq!(events[0].section(), "Gather");
    assert_eq!(events[2].section(), "Worker");
    assert_eq!(events[1].provenance(), &provenance("0", 12));
    assert_eq!(events[2].provenance(), &provenance("0.1", 0));
    assert_eq!(events[3].provenance().seq, 13);
}

#[test]
fn an_event_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Event>();
}
