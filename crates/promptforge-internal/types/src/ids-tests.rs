//! Tests for `ChainId`, `TaskId`, `TaskOrigin`, and `Provenance` rendering, parsing, and ordering.

use super::{ChainId, Provenance, TaskId, TaskOrigin};

#[test]
fn the_root_chain_is_zero_and_children_extend_it_by_index() {
    let root = ChainId::root();
    assert_eq!(root.to_string(), "0");
    assert_eq!(root.child(0).to_string(), "0.0");
    assert_eq!(root.child(2).child(1).to_string(), "0.2.1");
}

#[test]
fn a_section_entry_is_the_chain_id_extended_by_the_entry_index() {
    let chain = ChainId::root().child(3);
    assert_eq!(chain.entry(0), "0.3.0");
    assert_eq!(chain.entry(7), "0.3.7");
    // A parent and its child chain never share an entry id: the paths
    // differ in length.
    assert_ne!(ChainId::root().entry(3), chain.entry(0));
}

#[test]
fn a_rendered_id_parses_back_to_the_same_value() {
    let chain = ChainId::root().child(12).child(0);
    let parsed: ChainId = chain.to_string().parse().expect("a rendered id parses");
    assert_eq!(parsed, chain);
    let task: TaskId = "0.12.0".parse().expect("a task id parses");
    assert_eq!(task, TaskId::from(chain));
}

#[test]
fn malformed_paths_are_rejected() {
    for input in [
        "",
        ".",
        "0.",
        ".0",
        "0..1",
        "a",
        "0.-1",
        "0.+1",
        "0. 1",
        "99999999999",
    ] {
        let error = input
            .parse::<ChainId>()
            .expect_err("a malformed path is rejected");
        assert_eq!(error.input(), input);
    }
}

#[test]
fn ids_serialize_as_their_path_text() {
    let chain = ChainId::root().child(2);
    assert_eq!(
        serde_json::to_string(&chain).expect("a chain id serializes"),
        "\"0.2\""
    );
    let task: TaskId = serde_json::from_str("\"0.2\"").expect("a task id deserializes");
    assert_eq!(task, TaskId::from(chain));
    assert!(
        serde_json::from_str::<ChainId>("\"0.x\"").is_err(),
        "a malformed path fails to deserialize"
    );
}

#[test]
fn a_task_origin_round_trips_through_its_tag() {
    assert_eq!(TaskOrigin::Author.tag(), "author");
    assert_eq!(TaskOrigin::Model.tag(), "model");
    for origin in [TaskOrigin::Author, TaskOrigin::Model] {
        assert_eq!(TaskOrigin::from_tag(origin.tag()), Some(origin));
    }
    assert_eq!(
        TaskOrigin::from_tag("Author"),
        None,
        "the tag vocabulary is exact, never case-folded"
    );
    assert_eq!(
        serde_json::to_string(&TaskOrigin::Model).expect("an origin serializes"),
        "\"model\""
    );
}

#[test]
fn provenance_orders_by_task_then_sequence_and_round_trips() {
    let task: TaskId = "0.2".parse().expect("a task id parses");
    let first = Provenance {
        task: task.clone(),
        seq: 0,
    };
    let later = Provenance { task, seq: 7 };
    let other_task = Provenance {
        task: "0.3".parse().expect("a task id parses"),
        seq: 0,
    };
    assert!(first < later, "within one task the sequence orders");
    assert!(
        later < other_task,
        "the task path orders before the sequence"
    );
    assert_eq!(
        serde_json::to_string(&later).expect("provenance serializes"),
        r#"{"task":"0.2","seq":7}"#
    );
    assert_eq!(
        serde_json::from_str::<Provenance>(r#"{"task":"0.2","seq":7}"#)
            .expect("provenance deserializes"),
        later
    );
}
