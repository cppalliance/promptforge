//! Unit tests for the engine: building, resolving, shortlisting, rebuilding.

use std::sync::{Arc, OnceLock};

use promptforge_progress::{EventState, ProgressHub};
use serde_json::json;

use super::ToolPicker;
use crate::catalog::{Catalog, ToolDescriptor, ToolId};
use crate::config::Config;
use crate::embed::EMBEDDING_DIMENSIONS;
use crate::model::Model;
use crate::policy::Outcome;

/// One loaded model for the whole module's tests.
fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| Model::load().expect("the compiled-in model loads"))
}

/// A picker over `catalog` built with the shared model.
fn picker(catalog: Catalog, config: Config) -> ToolPicker {
    ToolPicker::build_with_model(model(), catalog, config, None).expect("the shared model indexes")
}

fn tiny_catalog() -> Catalog {
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

/// The same tool published twice, under the given servers.
fn republished(first: &str, second: &str) -> Catalog {
    let tool = ToolDescriptor::new(
        ToolId::new(first, "read_file"),
        "Read a file from disk",
        json!({"properties": {"path": {"type": "string"}}}),
    );
    let twin = ToolDescriptor::new(
        ToolId::new(second, "read_file"),
        "Read a file from disk",
        json!({"properties": {"path": {"type": "string"}}}),
    );
    Catalog::new(vec![tool, twin])
}

const fn assert_send_sync_static<T: Send + Sync + 'static>() {}

#[test]
fn a_picker_and_a_model_are_send_sync_static() {
    assert_send_sync_static::<ToolPicker>();
    assert_send_sync_static::<Model>();
}

#[test]
fn building_indexes_every_tool_as_a_unit_vector() {
    let catalog = tiny_catalog();
    let picker = picker(catalog.clone(), Config::default());
    assert_eq!(picker.len(), catalog.len());
    assert!(!picker.is_empty());
    assert_eq!(picker.iter().count(), catalog.len());
    assert_eq!(picker.config(), &Config::default());
    assert_eq!(picker.row(picker.len()), None);
    for index in 0..picker.len() {
        let row = picker.row(index).expect("every indexed tool has a row");
        assert_eq!(row.len(), EMBEDDING_DIMENSIONS);
        let norm = row.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "row {index} has length {norm}");
    }
    assert_ne!(picker.row(0), picker.row(1));
}

#[test]
fn an_empty_catalog_builds_a_picker_that_indexes_nothing() {
    let picker = picker(Catalog::default(), Config::default());
    assert!(picker.is_empty());
    assert_eq!(picker.len(), 0);
    assert_eq!(picker.row(0), None);
    assert_eq!(
        picker.resolve("read a file").expect("resolve"),
        Outcome::Absent
    );
}

#[test]
fn build_loads_a_model_and_reports_a_readable_debug() {
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).expect("build");
    let debug = format!("{picker:?}");
    assert!(debug.contains("ToolPicker"));
    assert!(
        !debug.contains("0.0,"),
        "the vectors must not be printed: {debug}"
    );
}

#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
#[test]
fn build_with_model_drives_the_leaf_to_one_in_tool_count_steps() {
    let hub = Arc::new(ProgressHub::new());
    let mut events = hub.subscribe();
    let tree = hub.operation();
    let leaf = tree.register("embed-tools", 1.0);
    assert!(matches!(
        events.try_recv().expect("register emits Begun").state,
        EventState::Begun { .. }
    ));

    let catalog = tiny_catalog();
    let picker =
        ToolPicker::build_with_model(model(), catalog.clone(), Config::default(), Some(&leaf))
            .expect("the shared model indexes");
    assert_eq!(picker.len(), catalog.len());
    assert_eq!(leaf.fraction(), 1.0, "indexing completes the leaf");

    let mut fractions = Vec::new();
    let mut saw_finished = false;
    while let Ok(event) = events.try_recv() {
        match event.state {
            EventState::Updated { fraction } => fractions.push(fraction),
            EventState::Finished { ok } => saw_finished |= ok,
            _ => {}
        }
    }
    assert_eq!(
        fractions,
        vec![0.5, 1.0],
        "one fraction step per embedded tool"
    );
    assert!(saw_finished, "completion emits Finished");
}

#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
#[test]
fn build_with_model_over_an_empty_catalog_completes_the_leaf() {
    let hub = Arc::new(ProgressHub::new());
    let tree = hub.operation();
    let leaf = tree.register("embed-tools", 1.0);

    let picker =
        ToolPicker::build_with_model(model(), Catalog::default(), Config::default(), Some(&leaf))
            .expect("an empty catalog builds");
    assert!(picker.is_empty());
    assert_eq!(leaf.fraction(), 1.0, "completion does not wait for tools");
}

#[test]
fn a_need_restating_one_tool_binds_that_tool() {
    let catalog = tiny_catalog();
    let picker = picker(catalog.clone(), Config::default());
    match picker
        .resolve("read the contents of a file from disk")
        .expect("resolve")
    {
        Outcome::Bind(tool) => assert_eq!(tool.name(), "read_file"),
        other => panic!("expected a bind, got {other:?}"),
    }
}

#[test]
fn a_need_no_tool_covers_abstains_and_shortlists_nothing() {
    let picker = picker(tiny_catalog(), Config::default());
    let need = "compose a haiku about the sorrow of autumn rain";
    assert_eq!(picker.resolve(need).expect("resolve"), Outcome::Absent);
    assert!(picker.shortlist(need, 5).expect("shortlist").is_empty());
}

#[test]
fn a_tool_republished_on_one_server_is_a_duplicate_and_across_two_is_ambiguous() {
    let same = picker(republished("files", "files"), Config::default());
    let need = "read a file from disk";
    assert!(matches!(
        same.resolve(need).expect("resolve"),
        Outcome::Duplicate(_)
    ));

    let across = picker(republished("files", "blobs"), Config::default());
    assert!(matches!(
        across.resolve(need).expect("resolve"),
        Outcome::Ambiguous(_)
    ));
}

#[test]
fn a_zero_limit_shortlist_is_empty_without_embedding() {
    let picker = picker(tiny_catalog(), Config::default());
    assert!(
        picker
            .shortlist("read a file", 0)
            .expect("shortlist")
            .is_empty()
    );
}

#[test]
fn near_duplicates_reuses_the_indexed_vectors_inclusively() {
    let catalog = republished("files", "blobs");
    let picker = picker(catalog.clone(), Config::default());
    let ids = vec![
        catalog.iter().nth(1).unwrap().id().clone(),
        catalog.iter().next().unwrap().id().clone(),
        catalog.iter().nth(1).unwrap().id().clone(),
    ];
    let pairs = picker.near_duplicates(&ids).expect("analysis");
    assert_eq!(pairs.len(), 1);
    let pair = pairs.get(0).expect("one pair");
    assert_eq!(pair.first().server(), "files");
    assert_eq!(pair.second().server(), "blobs");
    assert!(pair.similarity() >= picker.config().duplicate_threshold());
}

#[test]
fn near_duplicates_rejects_an_absent_identity() {
    let picker = picker(tiny_catalog(), Config::default());
    let missing = ToolId::new("missing", "tool");
    let error = picker
        .near_duplicates(std::slice::from_ref(&missing))
        .expect_err("absent");
    assert_eq!(error.missing_id(), &missing);
}

#[test]
fn get_returns_the_first_matching_descriptor() {
    let picker = picker(tiny_catalog(), Config::default());
    assert_eq!(
        picker
            .get(&ToolId::new("net", "fetch_url"))
            .map(ToolDescriptor::name),
        Some("fetch_url")
    );
    assert_eq!(picker.get(&ToolId::new("net", "absent")), None);
}

#[test]
fn a_rebuild_shares_the_model_and_preserves_the_exact_configuration() {
    let config = Config::default()
        .with_similarity_floor(0.7)
        .and_then(|config| config.with_top_k(4))
        .expect("valid overrides");
    let picker = picker(tiny_catalog(), config.clone());
    let rebuilt = picker
        .rebuild(republished("files", "blobs"))
        .expect("rebuild");
    assert!(
        picker.shares_model(&rebuilt),
        "a rebuild must not reload the model"
    );
    assert_eq!(rebuilt.config(), &config);
    assert_eq!(rebuilt.len(), 2);
    // The original is immutable and still answers from its own catalog.
    assert_eq!(picker.len(), 2);
}

#[test]
fn two_pickers_over_one_model_embed_identical_text_identically() {
    let first = picker(tiny_catalog(), Config::default());
    let second = picker(tiny_catalog(), Config::default());
    for index in 0..first.len() {
        assert_eq!(first.row(index), second.row(index), "row {index} differs");
    }
}
