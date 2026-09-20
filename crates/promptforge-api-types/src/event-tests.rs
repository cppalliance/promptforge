use serde_json::json;

use super::Event;
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
    // Lifecycle, message-carrying.
    round_trips(&Event::Lua {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: provenance("0", 1),
        message: "checkpoint".to_owned(),
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
        metrics: Some(CallMetrics {
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
        }),
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
