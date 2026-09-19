use std::sync::Arc;

use promptforge_api_types::event::Event;
use promptforge_api_types::ids::{AbandonReason, ChainId, TaskId, TaskOrigin};

use super::{Emitter, EventSink, unit_observation};
use crate::observe::{Observation, Observer, detail};

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
    walk.report("A", detail::SECTION_STARTED);
    arm.report("W", detail::SECTION_STARTED);
    walk.report("A", detail::LUA_CHUNK_STARTED);
    arm.report("W", detail::LUA_CHUNK_STARTED);
    arm.report("W", detail::LUA_CHUNK_SUCCEEDED);

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
fn a_lifecycle_report_becomes_the_matching_event_with_its_coordinates() {
    let sink = EventSink::default();
    let walk = emitter(&sink, root());
    walk.report("Gather", detail::STORE_WRITE_SUCCEEDED);
    let events = sink.take();
    assert_eq!(
        events,
        vec![Event::StoreWriteSucceeded {
            execution: "run-1".to_owned(),
            section: "Gather".to_owned(),
            provenance: promptforge_api_types::ids::Provenance {
                task: root(),
                seq: 0
            },
        }]
    );
    assert_eq!(
        unit_observation(&events[0]),
        Some(Observation::StoreWriteSucceeded),
        "the pair list maps back to the observation"
    );
}

#[test]
fn payload_variants_cross_field_for_field_and_unknown_ones_land_as_other() {
    let sink = EventSink::default();
    let walk = emitter(&sink, root());
    let task: TaskId = "0.3".parse().expect("a task id parses");
    walk.report(
        "Spawner",
        Observation::TaskStarted {
            task: task.clone(),
            target: "Worker".to_owned(),
            origin: TaskOrigin::Author,
            input: Some("in".to_owned()),
            item: Some(serde_json::json!("x")),
            index: Some(2),
            var: serde_json::json!({ "k": 1 }),
        },
    );
    walk.report(
        "Worker",
        Observation::TaskAbandoned {
            task: task.clone(),
            reason: AbandonReason::OwnerFailed,
        },
    );
    walk.observe("run-1", "Worker", Observation::Lua("checkpoint".to_owned()));
    walk.observe(
        "run-1",
        "Worker",
        Observation::Other("free-form".to_owned()),
    );
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
    assert!(matches!(&events[3], Event::Other { message, .. } if message == "free-form"));
    assert_eq!(unit_observation(&events[2]), None);
}

#[test]
fn the_observer_seam_routes_content_reports_into_the_buffer() {
    let sink = EventSink::default();
    let walk = emitter(&sink, root());
    let observer: &dyn Observer = &walk;
    observer.on_tool_result("run-1", "Chat", 7, 2, 3, "call_1", "echo", "out", true);
    observer.on_user_input("run-1", "Chat", "typed");
    let events = sink.take();
    assert!(matches!(
        &events[0],
        Event::ToolResult { turn: 3, tool_call_id, alias, content, trusted: true, .. }
            if tool_call_id == "call_1" && alias == "echo" && content == "out"
    ));
    assert!(matches!(&events[1], Event::UserInput { text, .. } if text == "typed"));
    assert_eq!(events[1].provenance().seq, 1);
}
