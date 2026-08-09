#![cfg(any())]
//! Superseded API tests retained for historical comparison.
//!
//! Everything here is reachable from outside the crate: no private module is
//! touched, no test-only constructor is used, and every type named is one the
//! `use` at the top of the file imports. That is the point of the file - the
//! unit tests inside the crate can pass while the surface a dependent builds
//! against is missing an export or a signature it cannot call.
//!
//! One engine is built per test and each build loads the compiled-in model, so
//! the catalog is kept to two tools and the file to a few tests.

use std::sync::Arc;

use promptforge_tool_picker::{
    Catalog, Config, Embedder, Error, NearDuplicate, Outcome, ToolDescriptor, ToolId, ToolPicker,
};
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

    let pairs: Vec<NearDuplicate> = picker
        .near_duplicates(&[
            ToolId::new("net", "fetch_url"),
            ToolId::new("files", "read_file"),
        ])
        .unwrap();
    assert!(pairs.is_empty());
}

#[test]
fn selected_set_analysis_rejects_an_absent_id_and_accepts_repetition() {
    let tools = catalog();
    let picker = ToolPicker::build(tools.clone(), Config::default()).unwrap();
    let missing = ToolId::new("missing", "tool");
    let error = picker
        .near_duplicates(&[
            tools.tools()[0].id.clone(),
            tools.tools()[0].id.clone(),
            missing.clone(),
        ])
        .expect_err("an absent identity must reject the complete selected set");

    assert!(
        matches!(error, Error::ToolNotInCatalog { ref id, .. } if *id == missing),
        "got {error:?}"
    );
    assert!(
        picker
            .near_duplicates(&[tools.tools()[0].id.clone(), tools.tools()[0].id.clone(),])
            .unwrap()
            .is_empty()
    );
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
fn a_caller_holding_one_encoder_can_build_an_engine_per_catalog() {
    // The whole point of the constructor: one model load, several engines. A
    // caller with a catalog that changes rebuilds instead of reloading.
    let embedder = Arc::new(Embedder::new().unwrap());
    let files =
        ToolPicker::build_with(Arc::clone(&embedder), catalog(), Config::default()).unwrap();

    let weather = Catalog::new(vec![ToolDescriptor::new(
        ToolId::new("weather", "get_forecast"),
        "Get the weather forecast for a city",
        json!({"properties": {"city": {"type": "string"}}}),
    )]);
    let forecasts =
        ToolPicker::build_with(Arc::clone(&embedder), weather.clone(), Config::default()).unwrap();

    match forecasts
        .resolve("get the weather forecast for a city")
        .unwrap()
    {
        Outcome::Bind(tool) => assert_eq!(tool.id, ToolId::new("weather", "get_forecast")),
        outcome => panic!("expected a binding, got {outcome:?}"),
    }
    // Each engine answers from its own catalog alone.
    assert_eq!(
        forecasts.resolve("read a file from disk").unwrap(),
        Outcome::Absent
    );
    assert_eq!(files.tools(), catalog().tools());

    // A rebuild is the same path reached from an engine rather than an `Arc`.
    let rebuilt = files.rebuild(weather).unwrap();
    assert_eq!(rebuilt.tools(), forecasts.tools());
    assert_eq!(rebuilt.vector(0), forecasts.vector(0));
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
