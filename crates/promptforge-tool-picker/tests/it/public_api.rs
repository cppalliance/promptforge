//! The crate as a caller sees it, through the public API and nothing else.
//!
//! One model is loaded per test binary and shared, so the suite pays for the
//! weights once however many pickers it builds.

use std::sync::OnceLock;

use promptforge_tool_picker::{
    BuildError, Catalog, Config, ConfigError, ConfigField, Model, ModelLoadError, Outcome,
    QueryError, SelectionError, ToolDescriptor, ToolId, ToolPicker,
};
use serde_json::json;

/// The one loaded model for this test binary.
fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| match Model::load() {
        Ok(model) => model,
        Err(error) => panic!("the compiled-in model must load: {error}"),
    })
}

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

const fn assert_send_sync_static<T: Send + Sync + 'static>() {}

#[test]
fn public_types_are_send_sync_static() {
    assert_send_sync_static::<Model>();
    assert_send_sync_static::<ToolPicker>();
    assert_send_sync_static::<BuildError>();
    assert_send_sync_static::<ModelLoadError>();
    assert_send_sync_static::<QueryError>();
    assert_send_sync_static::<SelectionError>();
    assert_send_sync_static::<ConfigError>();
}

#[test]
fn a_caller_builds_resolves_and_shortlists_through_the_public_api() {
    let picker = ToolPicker::build_with_model(model(), catalog(), Config::default(), None)
        .expect("index the catalog");
    assert_eq!(picker.len(), 2);
    assert_eq!(picker.iter().count(), 2);

    match picker.resolve("read a file from disk").expect("resolve") {
        Outcome::Bind(tool) => assert_eq!(tool.id(), &ToolId::new("files", "read_file")),
        outcome => panic!("expected a binding, got {outcome:?}"),
    }

    assert_eq!(
        picker
            .resolve("send an email to the team")
            .expect("resolve"),
        Outcome::Absent
    );

    let listed = picker
        .shortlist("fetch a web page over HTTP", 2)
        .expect("shortlist");
    assert_eq!(
        listed.first().map(ToolDescriptor::id),
        Some(&ToolId::new("net", "fetch_url"))
    );
    assert!(listed.len() <= 2);

    let pairs = picker
        .near_duplicates(&[
            ToolId::new("net", "fetch_url"),
            ToolId::new("files", "read_file"),
        ])
        .expect("selected analysis");
    assert!(pairs.is_empty());
}

#[test]
fn a_selection_error_names_the_first_missing_identity() {
    let picker = ToolPicker::build_with_model(model(), catalog(), Config::default(), None)
        .expect("index the catalog");
    let missing = ToolId::new("missing", "tool");
    let error = picker
        .near_duplicates(&[ToolId::new("files", "read_file"), missing.clone()])
        .expect_err("an absent identity rejects the selected set");
    assert_eq!(error.missing_id(), &missing);
}

#[test]
fn one_model_builds_a_picker_per_catalog_and_rebuild_reuses_it() {
    let files = ToolPicker::build_with_model(model(), catalog(), Config::default(), None)
        .expect("index the catalog");
    let weather = Catalog::new(vec![ToolDescriptor::new(
        ToolId::new("weather", "get_forecast"),
        "Get the weather forecast for a city",
        json!({"properties": {"city": {"type": "string"}}}),
    )]);
    let forecasts = ToolPicker::build_with_model(model(), weather.clone(), Config::default(), None)
        .expect("index a second catalog");

    match forecasts
        .resolve("get the weather forecast for a city")
        .expect("resolve")
    {
        Outcome::Bind(tool) => assert_eq!(tool.id(), &ToolId::new("weather", "get_forecast")),
        outcome => panic!("expected a binding, got {outcome:?}"),
    }
    assert_eq!(
        forecasts.resolve("read a file from disk").expect("resolve"),
        Outcome::Absent
    );

    let rebuilt = files.rebuild(weather).expect("rebuild");
    assert_eq!(rebuilt.len(), forecasts.len());
}

#[test]
fn the_same_need_answers_the_same_way_every_time() {
    let picker = ToolPicker::build_with_model(model(), catalog(), Config::default(), None)
        .expect("index the catalog");
    let first = picker.resolve("read a file from disk").expect("resolve");
    let listed = picker
        .shortlist("read a file from disk", 2)
        .expect("shortlist");
    for _ in 0..3 {
        assert_eq!(
            picker.resolve("read a file from disk").expect("resolve"),
            first
        );
        assert_eq!(
            picker
                .shortlist("read a file from disk", 2)
                .expect("shortlist"),
            listed
        );
    }
}

#[test]
fn configuration_setters_reject_out_of_domain_values() {
    let error: ConfigError = Config::default()
        .with_similarity_floor(2.0)
        .expect_err("out of domain");
    assert_eq!(error.field(), ConfigField::SimilarityFloor);
    assert!(Config::default().with_top_k(0).is_err());
}
