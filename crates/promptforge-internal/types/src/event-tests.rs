//! Tests for `Event` serde round trips and coordinate exposure.

use serde_json::json;

use super::Event;
use super::ReplyOrigin;
use crate::ids::{AbandonReason, Provenance, TaskId, TaskOrigin};
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

#[test]
fn one_variant_of_each_group_round_trips_through_serde() {
    // Lifecycle, payload-free.
    round_trips(&Event::RunStarted {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 0),
    });
    // Lifecycle, with a message.
    round_trips(&Event::Lua {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 1),
        message: "checkpoint".to_owned(),
    });
    round_trips(&Event::ModelMetadataDegraded {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 8),
        turn: 2,
        message: "malformed `usage` in completion response ignored: invalid type: string \"lots\", expected u64".to_owned(),
    });
    // Task, with the spawn seeds.
    round_trips(&Event::TaskStarted {
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
    });
    round_trips(&Event::TaskAbandoned {
        execution: "run-1".to_owned(),
        section: "Worker".to_owned(),
        provenance: provenance("0.0", 4),
        task: task("0.0"),
        reason: AbandonReason::ToolLoopExhausted,
    });
    // Content.
    round_trips(&Event::AssistantReply {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 3),
        turn: 2,
        text: "hello".to_owned(),
        finish_reason: Some("stop".to_owned()),
        model: "llama-3".to_owned(),
        metrics: Some(sample_metrics()),
        origin: ReplyOrigin::Chat,
    });
    round_trips(&Event::AssistantReply {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 7),
        turn: 2,
        text: "hello".to_owned(),
        finish_reason: Some("stop".to_owned()),
        model: "llama-3".to_owned(),
        metrics: Some(sample_metrics()),
        origin: ReplyOrigin::Infer,
    });
    round_trips(&Event::AssistantToolCalls {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 4),
        turn: 2,
        model: "llama-3".to_owned(),
        calls: vec![ToolCallEvent {
            id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            arguments: json!({ "path": "notes.txt" }),
        }],
    });
    round_trips(&Event::ToolResult {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 5),
        turn: 2,
        tool_call_id: "call_1".to_owned(),
        alias: "read_file".to_owned(),
        content: "file contents".to_owned(),
        trusted: false,
    });
    // Debug.
    round_trips(&Event::Response {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 6),
        turn: 2,
        body: json!({ "choices": [] }),
        finish_reason: Some("length".to_owned()),
        reasoning_content: None,
    });
}

#[test]
fn a_reply_origin_defaults_to_chat() {
    assert_eq!(ReplyOrigin::default(), ReplyOrigin::Chat);
}

#[test]
fn a_reply_serializes_its_origin() {
    let event = Event::AssistantReply {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 20),
        turn: 1,
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
}

#[test]
fn an_older_reply_without_origin_reads_back_as_chat() {
    // Backward compatibility: a log line written before `origin` existed
    // must still parse, defaulting to a chat reply. Without
    // `#[serde(default)]` this deserialization fails.
    let line = r#"{"kind":"assistant_reply","execution":"run-1","section":"Gather","provenance":{"task":"0","seq":1},"turn":1,"text":"hi","finish_reason":null,"model":"llama-3","metrics":null}"#;
    let event: Event = serde_json::from_str(line).expect("an old reply parses");
    match event {
        Event::AssistantReply { origin, .. } => assert_eq!(origin, ReplyOrigin::Chat),
        other => panic!("expected an assistant reply, got {other:?}"),
    }
}

#[test]
fn a_serialized_event_is_tagged_by_kind_with_its_coordinates_beside_the_payload() {
    // The tag and the three coordinates are the log schema the harness
    // writes `task_id` and `task_seq` from without inspecting the payload;
    // renaming any of them breaks every log written before it.
    let event = Event::StoreWriteSucceeded {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0.2", 9),
    };
    assert_eq!(
        serde_json::to_string(&event).expect("an event serializes"),
        r#"{"kind":"store_write_succeeded","execution":"run-1","section":"Gather","provenance":{"task":"0.2","seq":9}}"#
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
        Event::UserInput {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: provenance("0", 12),
            text: "hello".to_owned(),
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
