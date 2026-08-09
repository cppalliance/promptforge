//! Public contracts of the redesigned resolver API.

use promptforge_tool_picker::{
    Catalog, Config, ConfigField, Model, Outcome, ToolAnnotations, ToolDescriptor, ToolId,
    ToolPicker,
};
use serde_json::json;

fn catalog() -> Catalog {
    Catalog::from(vec![
        ToolDescriptor::new(
            ToolId::new("files", "read_file"),
            "Read a file from disk",
            json!({"properties": {"path": {"type": "string"}}}),
        )
        .with_annotations(ToolAnnotations::new().with_read_only(true)),
        ToolDescriptor::new(
            ToolId::new("net", "fetch_url"),
            "Fetch a web page over HTTP",
            json!({"properties": {"url": {"type": "string"}}}),
        ),
    ])
}

#[test]
fn configuration_is_valid_by_construction_and_checked_on_wire() {
    let config = Config::default()
        .with_similarity_floor(0.8)
        .unwrap()
        .with_top_k(2)
        .unwrap();
    assert_eq!(config.similarity_floor().to_bits(), 0.8_f32.to_bits());
    assert_eq!(config.top_k().get(), 2);
    assert_eq!(
        Config::default().with_top_k(0).unwrap_err().field(),
        ConfigField::TopK
    );
    assert!(serde_json::from_str::<Config>(r#"{"margin":null}"#).is_err());
    assert!(serde_json::from_str::<Config>(r#"{"solo_floor":1.1}"#).is_err());
}

#[test]
fn catalog_supports_all_three_iteration_modes_and_structural_ids() {
    let left = ToolId::new("a", "b\u{1f}c");
    let right = ToolId::new("a\u{1f}b", "c");
    assert_ne!(left, right);

    let mut catalog = catalog();
    assert_eq!(catalog.iter().count(), 2);
    assert_eq!((&catalog).into_iter().count(), 2);
    assert_eq!((&mut catalog).into_iter().count(), 2);
    assert_eq!(
        catalog
            .get(&ToolId::new("files", "read_file"))
            .unwrap()
            .name(),
        "read_file"
    );
}

#[test]
fn results_borrow_the_exact_catalog_descriptors() {
    let picker = ToolPicker::build(catalog(), Config::default()).unwrap();
    let resolved = picker.resolve("read a file from disk").unwrap();
    let Outcome::Bind(tool) = resolved else {
        panic!("the file need must bind");
    };
    let indexed = picker.get(&ToolId::new("files", "read_file")).unwrap();
    assert!(std::ptr::eq(tool, indexed));

    let shortlist = picker.shortlist("read a file from disk", 2).unwrap();
    assert_eq!(shortlist.len(), 1);
    assert!(std::ptr::eq(shortlist.first().unwrap(), indexed));
}

#[test]
fn one_model_builds_multiple_pickers_and_rebuild_preserves_policy() {
    let model = Model::load().unwrap();
    let config = Config::default().with_margin(0.1).unwrap();
    let first = ToolPicker::build_with_model(&model, catalog(), config.clone()).unwrap();
    let second = ToolPicker::build_with_model(&model, Catalog::default(), config.clone()).unwrap();
    assert_eq!(first.config(), &config);
    assert!(second.is_empty());
    assert_eq!(first.rebuild(Catalog::default()).unwrap().config(), &config);
}

#[test]
fn selected_scope_reports_the_first_missing_identity() {
    let picker = ToolPicker::build(catalog(), Config::default()).unwrap();
    let missing = ToolId::new("missing", "first");
    let error = picker
        .near_duplicates(&[missing.clone(), ToolId::new("missing", "second")])
        .unwrap_err();
    assert_eq!(error.missing_id(), &missing);
}
