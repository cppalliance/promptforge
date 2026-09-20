use std::sync::Arc;

use super::{Emitter, EventSink};
use crate::event::{Event, lifecycle};
use crate::ids::{AbandonReason, ChainId, Provenance, TaskId, TaskOrigin};

fn root() -> TaskId {
    TaskId::from(ChainId::root())
}

fn emitter(sink: &EventSink, task: TaskId) -> Emitter {
    Emitter::new(sink.clone(), task, Arc::from("run-1"), false)
}

#[test]
fn each_task_counts_its_own_sequence_from_zero() {
    let sink = EventSink::default();
    let walk = emitter(&sink, root());
    let arm = walk.for_task("0.0".parse().expect("a task id parses"));
    walk.report("A", lifecycle::SECTION_STARTED);
    arm.report("W", lifecycle::SECTION_STARTED);
    walk.report("A", lifecycle::LUA_CHUNK_STARTED);
    arm.report("W", lifecycle::LUA_CHUNK_STARTED);
    arm.report("W", lifecycle::LUA_CHUNK_SUCCEEDED);

    let stamps: Vec<(String, u32)> = sink
        .take()
        .iter()
        .map(|event| {
            let provenance = event.provenance();
            (provenance.task.to_string(), provenance.seq)
        })
        .collect();
    assert_eq!(
        stamps,
        vec![
            ("0".to_owned(), 0),
            ("0.0".to_owned(), 0),
            ("0".to_owned(), 1),
            ("0.0".to_owned(), 1),
            ("0.0".to_owned(), 2),
        ],
        "interleaved tasks keep independent dense sequences"
    );
    assert!(sink.take().is_empty(), "a drain empties the buffer");
}

#[test]
fn a_seeded_sink_continues_the_root_sequence_and_leaves_other_tasks_at_zero() {
    let sink = EventSink::seeded(3);
    let walk = emitter(&sink, root());
    let arm = walk.for_task("0.0".parse().expect("a task id parses"));
    walk.report("A", lifecycle::SECTION_STARTED);
    let stamp = walk.stamp_effect();
    arm.report("W", lifecycle::SECTION_STARTED);

    let events = sink.take();
    assert_eq!(
        events[0].provenance().seq,
        3,
        "the root task's first stamp continues from the seed"
    );
    assert_eq!(stamp.seq, 4, "an effect stamp advances the seeded counter");
    assert_eq!(
        events[1].provenance().seq,
        0,
        "a spawned task's sequence is unaffected by the seed"
    );
}

#[test]
fn a_lifecycle_report_becomes_the_matching_event_with_its_coordinates() {
    let sink = EventSink::default();
    let walk = emitter(&sink, root());
    walk.report("Gather", lifecycle::STORE_WRITE_SUCCEEDED);
    assert_eq!(
        sink.take(),
        vec![Event::StoreWriteSucceeded {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: Provenance {
                task: root(),
                seq: 0
            },
        }]
    );
}

#[test]
fn an_effect_stamp_shares_the_task_sequence_with_its_events() {
    let sink = EventSink::default();
    let walk = emitter(&sink, root());
    walk.report("A", lifecycle::SECTION_STARTED);
    let stamp = walk.stamp_effect();
    walk.report("A", lifecycle::SECTION_FINISHED);
    let events = sink.take();
    assert_eq!(
        stamp.seq, 1,
        "the effect takes the sequence between the two events"
    );
    assert_eq!(events[0].provenance().seq, 0);
    assert_eq!(events[1].provenance().seq, 2);
}

#[test]
fn payload_variants_cross_field_for_field() {
    let sink = EventSink::default();
    let walk = emitter(&sink, root());
    let task: TaskId = "0.3".parse().expect("a task id parses");
    walk.emit("Spawner", |execution, section, provenance| {
        Event::TaskStarted {
            execution,
            section,
            provenance,
            task: task.clone(),
            target: "Worker".to_owned(),
            origin: TaskOrigin::Author,
            input: Some("in".to_owned()),
            item: Some(serde_json::json!("x")),
            index: Some(2),
            var: serde_json::json!({ "k": 1 }),
        }
    });
    walk.emit("Worker", |execution, section, provenance| {
        Event::TaskAbandoned {
            execution,
            section,
            provenance,
            task: task.clone(),
            reason: AbandonReason::OwnerFailed,
        }
    });
    walk.lua("Worker", "checkpoint");
    let events = sink.take();
    assert!(matches!(
        &events[0],
        Event::TaskStarted { task: started, target, origin: TaskOrigin::Author, input: Some(input), item: Some(_), index: Some(2), var, .. }
            if *started == task && target == "Worker" && input == "in" && var["k"] == 1
    ));
    assert!(matches!(
        &events[1],
        Event::TaskAbandoned { task: ended, reason: AbandonReason::OwnerFailed, .. } if *ended == task
    ));
    assert!(matches!(&events[2], Event::Lua { message, .. } if message == "checkpoint"));
}

#[test]
fn content_reports_land_in_the_buffer_in_order() {
    let sink = EventSink::default();
    let walk = emitter(&sink, root());
    walk.tool_result("Chat", 3, "call_1", "echo", "out", true);
    walk.user_input("Chat", "typed");
    let events = sink.take();
    assert!(matches!(
        &events[0],
        Event::ToolResult { turn: 3, tool_call_id, alias, content, trusted: true, .. }
            if tool_call_id == "call_1" && alias == "echo" && content == "out"
    ));
    assert!(matches!(&events[1], Event::UserInput { text, .. } if text == "typed"));
    assert_eq!(events[1].provenance().seq, 1);
}

#[test]
fn the_root_emitter_reports_under_task_zero_with_its_execution() {
    let sink = EventSink::default();
    let emitter = Emitter::root(sink.clone(), "parse-1", true);
    assert!(emitter.captures_debug());
    assert_eq!(emitter.execution(), "parse-1");
    assert_eq!(emitter.task(), &root());
    emitter.report("Prompt", lifecycle::PARSE_STARTED);
    let events = sink.take();
    assert_eq!(events[0].execution(), "parse-1");
    assert_eq!(events[0].provenance().task, root());
}
