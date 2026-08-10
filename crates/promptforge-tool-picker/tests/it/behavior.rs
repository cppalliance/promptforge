//! The four outcomes, end to end, over a catalog committed as data.
//!
//! The catalog is a JSON fixture compiled in and read through the same public
//! deserialization a caller would use. Each need falls out of the fixture's own
//! prose against the default thresholds; no score is injected.

use std::sync::OnceLock;

use promptforge_tool_picker::{Catalog, Config, Outcome, ToolId, ToolPicker};

/// The fixture catalog, as it is committed.
const MIXED_SERVERS: &str = include_str!("../fixtures/mixed-servers.json");

/// A need only `weather/get_forecast` covers.
const BIND_NEED: &str = "get the weather forecast for a city";
/// A need both of one server's copy-pasted calendar tools cover.
const DUPLICATE_NEED: &str =
    "create a new calendar event with a title, a start time and an end time";
/// A need two servers cover equally well.
const AMBIGUOUS_NEED: &str = "read the contents of a file from the local disk";
/// A need no tool in the catalog covers at all.
const ABSENT_NEED: &str = "translate this paragraph into Japanese";

const NEEDS: [&str; 4] = [BIND_NEED, DUPLICATE_NEED, AMBIGUOUS_NEED, ABSENT_NEED];

/// The fixture parsed into a catalog through the public serde support.
fn mixed_servers() -> Catalog {
    match serde_json::from_str(MIXED_SERVERS) {
        Ok(catalog) => catalog,
        Err(error) => panic!("the committed fixture must be a valid catalog: {error}"),
    }
}

/// The picker over the fixture, built once for the whole file.
fn picker() -> &'static ToolPicker {
    static PICKER: OnceLock<ToolPicker> = OnceLock::new();
    PICKER.get_or_init(
        || match ToolPicker::build(mixed_servers(), Config::default()) {
            Ok(picker) => picker,
            Err(error) => panic!("the fixture must build: {error}"),
        },
    )
}

#[test]
fn a_need_only_one_tool_covers_binds_that_tool() {
    match picker().resolve(BIND_NEED).expect("resolve") {
        Outcome::Bind(tool) => assert_eq!(tool.id(), &ToolId::new("weather", "get_forecast")),
        other => panic!("a need with one plain answer must bind, got {other:?}"),
    }
}

#[test]
fn one_servers_copy_pasted_pair_is_reported_as_a_duplicate() {
    let outcome = picker().resolve(DUPLICATE_NEED).expect("resolve");
    let Outcome::Duplicate(group) = &outcome else {
        panic!("one server's two names for one tool must be a duplicate, got {outcome:?}");
    };
    assert_eq!(group.first().id(), &ToolId::new("calendar", "create_event"));
    assert_eq!(group.second().id(), &ToolId::new("calendar", "add_event"));
}

#[test]
fn a_duplicate_is_decided_between_the_tools_and_not_between_their_scores() {
    let picker = picker();
    let calendar = picker
        .near_duplicates(&[
            ToolId::new("calendar", "create_event"),
            ToolId::new("calendar", "add_event"),
        ])
        .expect("selected analysis");
    assert_eq!(
        calendar.len(),
        1,
        "the copy-pasted calendar pair is a near-duplicate"
    );

    let files = picker
        .near_duplicates(&[
            ToolId::new("files", "read_file"),
            ToolId::new("blobs", "read_text_file"),
        ])
        .expect("selected analysis");
    assert!(
        files.is_empty(),
        "the short-description file pair sits below the duplicate threshold"
    );
}

#[test]
fn two_servers_publishing_one_capability_are_ambiguous() {
    let outcome = picker().resolve(AMBIGUOUS_NEED).expect("resolve");
    let Outcome::Ambiguous(group) = &outcome else {
        panic!("a near-tie the margin cannot separate must be a shortlist, got {outcome:?}");
    };
    assert_eq!(group.first().id(), &ToolId::new("files", "read_file"));
    assert_eq!(group.second().id(), &ToolId::new("blobs", "read_text_file"));
    assert_ne!(group.first().server(), group.second().server());
}

#[test]
fn a_need_the_catalog_does_not_cover_abstains_and_offers_nothing() {
    let picker = picker();
    assert_eq!(
        picker.resolve(ABSENT_NEED).expect("resolve"),
        Outcome::Absent
    );
    assert!(
        picker
            .shortlist(ABSENT_NEED, picker.len())
            .expect("shortlist")
            .is_empty(),
        "an abstention must not be contradicted by a shortlist of near-misses"
    );
}

#[test]
fn a_shortlist_offers_exactly_the_tools_the_decision_weighed() {
    let picker = picker();
    let listed = picker
        .shortlist(AMBIGUOUS_NEED, picker.len())
        .expect("shortlist");
    let Outcome::Ambiguous(group) = picker.resolve(AMBIGUOUS_NEED).expect("resolve") else {
        panic!("the ambiguous need must be a shortlist");
    };
    let listed_ids: Vec<ToolId> = listed.iter().map(|tool| tool.id().clone()).collect();
    let group_ids: Vec<ToolId> = group.iter().map(|tool| tool.id().clone()).collect();
    assert_eq!(listed_ids, group_ids);

    let bind = picker
        .shortlist(BIND_NEED, picker.len())
        .expect("shortlist");
    assert_eq!(bind.len(), 1);
    assert_eq!(
        bind.first().map(|tool| tool.id().clone()),
        Some(ToolId::new("weather", "get_forecast"))
    );
}

#[test]
fn two_builds_answer_every_need_identically() {
    let first = picker();
    let second = ToolPicker::build(mixed_servers(), Config::default())
        .expect("the fixture builds a second time");
    for need in NEEDS {
        assert_eq!(
            first.resolve(need).expect("resolve"),
            second.resolve(need).expect("resolve"),
            "two builds disagreed about {need:?}"
        );
    }
}

#[test]
fn repeating_a_need_answers_the_same_way_every_time() {
    let picker = picker();
    for need in NEEDS {
        let outcome = picker.resolve(need).expect("resolve");
        for _ in 0..4 {
            assert_eq!(
                picker.resolve(need).expect("resolve"),
                outcome,
                "{need:?} drifted"
            );
        }
    }
}
