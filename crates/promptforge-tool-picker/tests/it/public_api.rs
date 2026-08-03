//! The crate as a caller sees it, through the public API and nothing else.
//!
//! Everything here is reachable from outside the crate: no private module is
//! touched, no test-only constructor is used, and every type named is one the
//! `use` at the top of the file imports. That is the point of the file - the
//! unit tests inside the crate can pass while the surface a dependent builds
//! against is missing an export or a signature it cannot call.
//!
//! One engine is built per test and each build loads the compiled-in model, so
//! the catalog is kept to two tools and the file to a few tests.

use promptforge_tool_picker::{Catalog, Config, Outcome, ToolDescriptor, ToolId, ToolPicker};
use serde_json::json;

/// Two plainly unrelated tools: enough to bind one and to miss both.
fn catalog() -> Catalog {
    Catalog::new(vec![
        ToolDescriptor::new(
            ToolId::new("files", "read_file"),
            "Read a file from disk",
            json!({"properties": {"path": {"type": "string"}}}),
        ),
        ToolDescriptor::new(
            ToolId::new("net", "fetch_url"),
            "Fetch a web page over HTTP",
            json!({"properties": {"url": {"type": "string"}}}),
        ),
    ])
}

#[test]
fn a_caller_can_build_resolve_and_shortlist_through_the_public_api() {
    let tools = catalog();
    let picker = ToolPicker::build(tools.clone(), Config::default()).unwrap();
    assert_eq!(picker.len(), 2);
    assert_eq!(picker.tools(), tools.tools());

    // A need that restates one tool binds to that tool.
    match picker.resolve("read a file from disk").unwrap() {
        Outcome::Bind(tool) => assert_eq!(tool.id, ToolId::new("files", "read_file")),
        outcome => panic!("expected a binding, got {outcome:?}"),
    }

    // A need nothing in the catalog covers is an abstention, not an error.
    assert_eq!(
        picker.resolve("send an email to the team").unwrap(),
        Outcome::Absent
    );

    // A shortlist reports candidates rather than deciding between them.
    let candidates = picker.shortlist("fetch a web page over HTTP", 2).unwrap();
    assert_eq!(candidates[0].id, ToolId::new("net", "fetch_url"));
    assert!(candidates.len() <= 2);
}

#[test]
fn a_shortlist_offers_nothing_for_a_need_that_resolves_absent() {
    let picker = ToolPicker::build(catalog(), Config::default()).unwrap();
    let need = "send an email to the team";
    assert_eq!(picker.resolve(need).unwrap(), Outcome::Absent);
    assert!(
        picker.shortlist(need, 2).unwrap().is_empty(),
        "a shortlist must not offer candidates the engine declined to match"
    );
}

#[test]
fn the_same_need_answers_the_same_way_every_time() {
    let picker = ToolPicker::build(catalog(), Config::default()).unwrap();
    let first = picker.resolve("read a file from disk").unwrap();
    let listed = picker.shortlist("read a file from disk", 2).unwrap();
    for _ in 0..3 {
        assert_eq!(picker.resolve("read a file from disk").unwrap(), first);
        assert_eq!(
            picker.shortlist("read a file from disk", 2).unwrap(),
            listed
        );
    }
}
