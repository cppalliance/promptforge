//! The four outcomes, end to end, over a catalog committed as data.
//!
//! The catalog under test is a JSON file in `tests/fixtures/`, and it is
//! compiled in rather than opened: a fixture is part of the crate, so nothing
//! here depends on a working directory, on a path outside the crate, or on
//! anything being fetched. It is read through the same public
//! deserialization a caller would use.
//!
//! Where the unit tests hand the policy scores that were written down, these
//! tests hand the whole engine plain-English needs and let the model produce
//! the scores. Every outcome below therefore falls out of the fixture's own
//! prose against the default thresholds - no score is injected and no
//! threshold is bent to reach a verdict.
//!
//! # The fixture is shaped to make each outcome inevitable
//!
//! Five tools across four servers, which is the smallest catalog that can
//! produce all four answers, because each answer needs a different relation
//! between tools:
//!
//! - `weather/get_forecast` has nothing like it in the catalog, so a need that
//!   restates it leaves every other tool far behind and **binds**.
//! - `calendar/create_event` and `calendar/add_event` are one server's two
//!   names for one capability, carrying the same description word for word -
//!   the copy-paste that happens when a tool is renamed and the old name is
//!   kept working. Their own embeddings sit at or above the default duplicate
//!   threshold, so a calendar need reports a **duplicate**.
//! - `files/read_file` and `blobs/read_text_file` are the same capability
//!   published by two different servers, which is the ordinary result of
//!   pointing one engine at overlapping catalogs. Neither server is at fault,
//!   and the margin cannot separate them, so a file-reading need is
//!   **ambiguous**.
//! - Nothing in the catalog translates anything, so a translation need is
//!   **absent**.
//!
//! # One engine, shared
//!
//! Building an engine loads the compiled-in model, which costs far more than
//! everything else in this file put together, so the engine is built once and
//! shared by every assertion. Determinism is the one property that cannot be
//! checked that way - it is a claim about two builds - so exactly one second
//! build exists, in the test that needs it.

use promptforge_tool_picker::{Catalog, Config, Outcome, ToolDescriptor, ToolId, ToolPicker};
use std::sync::OnceLock;

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

/// Every need above, so a property can be asserted over all four outcomes.
const NEEDS: [&str; 4] = [BIND_NEED, DUPLICATE_NEED, AMBIGUOUS_NEED, ABSENT_NEED];

/// The fixture parsed into a catalog through the public serde support.
fn mixed_servers() -> Catalog {
    match serde_json::from_str(MIXED_SERVERS) {
        Ok(catalog) => catalog,
        Err(error) => panic!("the committed fixture must be a valid catalog: {error}"),
    }
}

/// The engine over the fixture, built once for the whole file.
fn picker() -> &'static ToolPicker {
    static PICKER: OnceLock<ToolPicker> = OnceLock::new();
    PICKER.get_or_init(
        || match ToolPicker::build(mixed_servers(), Config::default()) {
            Ok(picker) => picker,
            Err(error) => panic!("the fixture must build: {error}"),
        },
    )
}

/// The identities of a reported group, in the order it reported them.
fn ids(group: &[ToolDescriptor]) -> Vec<ToolId> {
    group.iter().map(|tool| tool.id.clone()).collect()
}

/// The cosine similarity between two indexed tools' own embeddings.
///
/// Both stored rows are unit length, so the dot product is the cosine - the
/// same quantity `duplicate_threshold` is compared against.
fn tool_similarity(picker: &ToolPicker, left: &ToolId, right: &ToolId) -> f32 {
    let row = |id: &ToolId| {
        let Some(index) = picker.tools().iter().position(|tool| &tool.id == id) else {
            panic!("the fixture must contain {id:?}");
        };
        let Some(row) = picker.vector(index) else {
            panic!("every indexed tool has a stored row");
        };
        row
    };
    row(left).iter().zip(row(right)).map(|(a, b)| a * b).sum()
}

#[test]
fn a_need_only_one_tool_covers_binds_that_tool() {
    match picker().resolve(BIND_NEED).unwrap() {
        Outcome::Bind(tool) => assert_eq!(tool.id, ToolId::new("weather", "get_forecast")),
        other => panic!("a need with one plain answer must bind, got {other:?}"),
    }
}

#[test]
fn one_servers_copy_pasted_pair_is_reported_as_a_duplicate() {
    let outcome = picker().resolve(DUPLICATE_NEED).unwrap();
    let Outcome::Duplicate(group) = &outcome else {
        panic!("one server's two names for one tool must be a duplicate, got {outcome:?}");
    };
    assert_eq!(
        ids(group),
        vec![
            ToolId::new("calendar", "create_event"),
            ToolId::new("calendar", "add_event"),
        ],
        "the group is the leader and the twin it shares a server with"
    );
}

#[test]
fn a_duplicate_is_decided_between_the_tools_and_not_between_their_scores() {
    let picker = picker();
    let calendar = tool_similarity(
        picker,
        &ToolId::new("calendar", "create_event"),
        &ToolId::new("calendar", "add_event"),
    );
    assert!(
        calendar >= picker.config().duplicate_threshold,
        "the copy-pasted calendar pair is {calendar}, under the {} threshold that makes it a fault",
        picker.config().duplicate_threshold
    );

    // The two file tools are the same capability too, and their descriptions
    // are identical word for word - but their names differ by a whole word,
    // and their descriptions are one short line, so the difference is a large
    // fraction of the little text there is. They land below the threshold.
    // Whether a copy-paste is caught therefore depends on how much prose the
    // differing name is diluted by, which the calendar pair has plenty of.
    let files = tool_similarity(
        picker,
        &ToolId::new("files", "read_file"),
        &ToolId::new("blobs", "read_text_file"),
    );
    assert!(
        files < picker.config().duplicate_threshold,
        "the short-description file pair is {files}, at or over the threshold"
    );
}

#[test]
fn two_servers_publishing_one_capability_are_ambiguous() {
    let outcome = picker().resolve(AMBIGUOUS_NEED).unwrap();
    let Outcome::Ambiguous(group) = &outcome else {
        panic!("a near-tie the margin cannot separate must be a shortlist, got {outcome:?}");
    };
    assert_eq!(
        ids(group),
        vec![
            ToolId::new("files", "read_file"),
            ToolId::new("blobs", "read_text_file"),
        ],
        "both candidates are offered, best first, for a caller to choose between"
    );
    // Nothing is wrong with this catalog: the collision spans two servers, so
    // it is a fact about the union the caller assembled, not a fault in it.
    assert_ne!(group[0].server(), group[1].server());
}

#[test]
fn a_need_the_catalog_does_not_cover_abstains_and_offers_nothing() {
    let picker = picker();
    assert_eq!(picker.resolve(ABSENT_NEED).unwrap(), Outcome::Absent);
    assert!(
        picker
            .shortlist(ABSENT_NEED, picker.len())
            .unwrap()
            .is_empty(),
        "an abstention must not be contradicted by a shortlist of near-misses"
    );
}

#[test]
fn a_shortlist_offers_exactly_the_tools_the_decision_weighed() {
    let picker = picker();
    assert_eq!(
        picker.shortlist(AMBIGUOUS_NEED, picker.len()).unwrap(),
        match picker.resolve(AMBIGUOUS_NEED).unwrap() {
            Outcome::Ambiguous(group) => group,
            other => panic!("expected a shortlist, got {other:?}"),
        }
    );
    assert_eq!(
        ids(&picker.shortlist(BIND_NEED, picker.len()).unwrap()),
        vec![ToolId::new("weather", "get_forecast")],
        "only the bound tool clears the floor for this need"
    );
}

#[test]
fn two_engines_built_from_one_fixture_answer_every_need_identically() {
    let first = picker();
    let second = ToolPicker::build(mixed_servers(), Config::default())
        .expect("the fixture must build a second time");

    assert_eq!(first.tools(), second.tools());
    for index in 0..first.len() {
        assert_eq!(
            first.vector(index),
            second.vector(index),
            "row {index} differs between builds, so no later answer can be reproducible"
        );
    }

    for need in NEEDS {
        assert_eq!(
            first.resolve(need).unwrap(),
            second.resolve(need).unwrap(),
            "two builds disagreed about {need:?}"
        );
        assert_eq!(
            first.shortlist(need, first.len()).unwrap(),
            second.shortlist(need, second.len()).unwrap(),
            "two builds shortlisted {need:?} differently"
        );
    }
}

#[test]
fn repeating_a_need_on_one_engine_answers_the_same_way_every_time() {
    let picker = picker();
    for need in NEEDS {
        let outcome = picker.resolve(need).unwrap();
        let listed = picker.shortlist(need, picker.len()).unwrap();
        for _ in 0..4 {
            assert_eq!(picker.resolve(need).unwrap(), outcome, "{need:?} drifted");
            assert_eq!(
                picker.shortlist(need, picker.len()).unwrap(),
                listed,
                "the shortlist for {need:?} drifted"
            );
        }
    }
}
