//! Unit tests for the engine: building, ranking, resolving, and rebuilding.

use std::sync::Arc;

use super::ToolPicker;
use crate::catalog::{Catalog, ToolDescriptor, ToolId};
use crate::config::Config;
use crate::embed::{EMBEDDING_DIMENSIONS, Embedder};
use crate::error::Error;
use crate::policy::Outcome;
use serde_json::json;

/// Two tools: enough to prove rows are kept apart, cheap enough to embed.
///
/// Every build in this module loads the whole model, so each tool added
/// here is paid for by every test in the file.
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

#[test]
fn building_indexes_every_tool_as_a_unit_vector() {
    let catalog = tiny_catalog();
    let picker = ToolPicker::build(catalog.clone(), Config::default()).unwrap();

    assert_eq!(picker.len(), catalog.len());
    assert!(!picker.is_empty());
    assert_eq!(picker.tools(), catalog.tools());
    assert_eq!(picker.config(), &Config::default());
    assert_eq!(picker.vector(picker.len()), None);

    for index in 0..picker.len() {
        let vector = picker.vector(index).expect("every indexed tool has a row");
        assert_eq!(vector.len(), EMBEDDING_DIMENSIONS);
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "row {index} has length {norm}; a dot product is only a cosine at 1"
        );
    }

    // Distinct tools must not share a row, or the flat buffer is misread.
    assert_ne!(picker.vector(0), picker.vector(1));
}

/// How `build` and `validate` each reject one configuration, side by side.
///
/// Both verdicts come from the same value so a caller can be shown that
/// building adds nothing to the configuration check and subtracts nothing
/// from it. Because `build` validates first, neither call reaches the model.
fn rejections(config: &Config) -> (Error, Error) {
    let built = ToolPicker::build(tiny_catalog(), config.clone())
        .expect_err("an invalid configuration must not build an engine");
    let validated = config
        .validate()
        .expect_err("the same configuration must fail validation");
    (built, validated)
}

#[test]
fn a_zero_length_shortlist_is_rejected_as_validate_rejects_it() {
    let (built, validated) = rejections(&Config {
        top_k: 0,
        ..Config::default()
    });
    assert!(
        matches!(
            (&built, &validated),
            (Error::EmptyShortlist, Error::EmptyShortlist)
        ),
        "build gave {built:?} where validate gave {validated:?}"
    );
    assert_eq!(built.to_string(), validated.to_string());
}

#[test]
fn a_threshold_outside_the_cosine_range_is_rejected_as_validate_rejects_it() {
    let (built, validated) = rejections(&Config {
        margin: 1.5,
        ..Config::default()
    });
    assert!(
        matches!(
            (&built, &validated),
            (
                Error::ThresholdOutOfRange { .. },
                Error::ThresholdOutOfRange { .. }
            )
        ),
        "build gave {built:?} where validate gave {validated:?}"
    );
    assert_eq!(built.to_string(), validated.to_string());
}

#[test]
fn an_empty_catalog_builds_an_engine_that_indexes_nothing() {
    let picker = ToolPicker::build(Catalog::default(), Config::default()).unwrap();
    assert!(picker.is_empty());
    assert_eq!(picker.len(), 0);
    assert!(picker.tools().is_empty());
    assert_eq!(picker.vector(0), None);
}

#[test]
fn a_need_restating_a_tools_text_ranks_that_tool_first() {
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    for (index, need) in [
        (0, "read the contents of a file from disk"),
        (1, "fetch a web page over HTTP"),
    ] {
        let ranked = picker.rank(need, picker.len()).unwrap();
        assert_eq!(ranked.len(), picker.len());
        assert_eq!(
            ranked[0].index,
            index,
            "{need:?} ranked {:?} first",
            picker.tools()[ranked[0].index].name()
        );
        assert!(
            ranked[0].score > ranked[1].score,
            "the restated tool scored {} against the other's {}",
            ranked[0].score,
            ranked[1].score
        );
        for candidate in &ranked {
            assert!(
                (-1.0..=1.0).contains(&candidate.score),
                "score {} is outside the cosine range of unit vectors",
                candidate.score
            );
        }
    }
}

#[test]
fn a_shortlist_longer_than_the_catalog_is_not_padded() {
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    let ranked = picker.rank("read a file", 50).unwrap();
    assert_eq!(ranked.len(), picker.len());
}

#[test]
fn an_empty_index_ranks_nothing() {
    let picker = ToolPicker::build(Catalog::default(), Config::default()).unwrap();
    assert!(picker.rank("read a file", 3).unwrap().is_empty());
}

#[test]
fn ranking_the_same_need_twice_yields_the_same_order() {
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    let first = picker
        .rank("store a document somewhere", picker.len())
        .unwrap();
    let second = picker
        .rank("store a document somewhere", picker.len())
        .unwrap();
    assert_eq!(first, second);
}

/// The same tool published twice, under the given servers.
///
/// Identical text embeds to identical vectors, so the pair is
/// indistinguishable to any need - which is the situation both ambiguity
/// outcomes exist to report, differing only in the servers.
fn republished(first: &str, second: &str) -> Catalog {
    let tool = ToolDescriptor::new(
        ToolId::new(first, "read_file"),
        "Read a file from disk",
        json!({"properties": {"path": {"type": "string"}}}),
    );
    let mut twin = tool.clone();
    twin.id = ToolId::new(second, "read_file");
    Catalog::new(vec![tool, twin])
}

#[test]
fn a_need_restating_one_tool_binds_that_tool() {
    let catalog = tiny_catalog();
    let picker = ToolPicker::build(catalog.clone(), Config::default()).unwrap();
    let need = catalog.tools()[1].enriched_text();
    assert_eq!(
        picker.resolve(&need).unwrap(),
        Outcome::Bind(catalog.tools()[1].clone())
    );
}

#[test]
fn a_need_no_tool_covers_abstains() {
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    assert_eq!(
        picker
            .resolve("compose a haiku about the sorrow of autumn rain")
            .unwrap(),
        Outcome::Absent
    );
}

#[test]
fn a_tool_republished_across_servers_yields_a_shortlist() {
    let catalog = republished("files", "blobs");
    let picker = ToolPicker::build(catalog.clone(), Config::default()).unwrap();
    let need = catalog.tools()[0].enriched_text();
    assert_eq!(
        picker.resolve(&need).unwrap(),
        Outcome::Ambiguous(catalog.tools().to_vec())
    );
}

#[test]
fn one_server_publishing_the_same_tool_twice_is_reported_as_a_fault() {
    let catalog = republished("files", "files");
    let picker = ToolPicker::build(catalog.clone(), Config::default()).unwrap();
    let need = catalog.tools()[0].enriched_text();
    assert_eq!(
        picker.resolve(&need).unwrap(),
        Outcome::Duplicate(catalog.tools().to_vec())
    );
}

#[test]
fn a_shortlist_of_one_still_sees_the_runner_up() {
    // `top_k: 1` is a valid configuration, and ranking exactly one
    // candidate would hide every ambiguity: the twin would never be
    // ranked, and the server's fault would bind silently instead.
    let catalog = republished("files", "files");
    let config = Config {
        top_k: 1,
        ..Config::default()
    };
    assert!(config.validate().is_ok());
    let picker = ToolPicker::build(catalog.clone(), config).unwrap();
    let need = catalog.tools()[0].enriched_text();
    assert_eq!(
        picker.resolve(&need).unwrap(),
        Outcome::Duplicate(catalog.tools().to_vec())
    );
}

#[test]
fn deciding_the_same_need_twice_yields_the_same_outcome() {
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    let first = picker.resolve("read a file from disk").unwrap();
    for _ in 0..4 {
        assert_eq!(picker.resolve("read a file from disk").unwrap(), first);
    }
}

#[test]
fn a_shortlist_offers_the_matching_tools_best_first() {
    let catalog = tiny_catalog();
    let picker = ToolPicker::build(catalog.clone(), Config::default()).unwrap();
    let need = catalog.tools()[1].enriched_text();
    assert_eq!(
        picker.shortlist(&need, 2).unwrap(),
        vec![catalog.tools()[1].clone()],
        "the unrelated tool is below the floor and is not offered"
    );
}

#[test]
fn a_shortlist_is_empty_exactly_where_resolution_abstains() {
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    let need = "compose a haiku about the sorrow of autumn rain";
    assert_eq!(picker.resolve(need).unwrap(), Outcome::Absent);
    assert!(picker.shortlist(need, 5).unwrap().is_empty());
}

#[test]
fn a_shortlist_takes_its_length_from_k_and_not_from_the_config() {
    // `top_k` bounds the shortlist a resolution reports; `k` is what this
    // caller asked for on this call, and nothing clamps one to the other.
    let catalog = republished("files", "blobs");
    let config = Config {
        top_k: 1,
        ..Config::default()
    };
    let picker = ToolPicker::build(catalog.clone(), config).unwrap();
    let need = catalog.tools()[0].enriched_text();
    assert_eq!(
        picker.shortlist(&need, 2).unwrap(),
        catalog.tools().to_vec()
    );
    assert_eq!(
        picker.shortlist(&need, 1).unwrap(),
        vec![catalog.tools()[0].clone()]
    );
    assert!(picker.shortlist(&need, 0).unwrap().is_empty());
    // Asking for more than there is returns what there is, unpadded.
    assert_eq!(
        picker.shortlist(&need, 99).unwrap(),
        catalog.tools().to_vec()
    );
}

#[test]
fn an_empty_index_abstains() {
    let picker = ToolPicker::build(Catalog::default(), Config::default()).unwrap();
    assert_eq!(picker.resolve("read a file").unwrap(), Outcome::Absent);
}

#[test]
fn a_picker_can_be_shared_across_threads() {
    // The type callers put behind an `Arc`. It holds the `Embedder`, and
    // through it two external crates' types, so a dependency upgrade could
    // take `Send` or `Sync` away without changing a signature here.
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolPicker>();
}

/// One tool, plainly unlike anything in [`tiny_catalog`].
fn other_catalog() -> Catalog {
    Catalog::new(vec![ToolDescriptor::new(
        ToolId::new("weather", "get_forecast"),
        "Get the weather forecast for a city",
        json!({"properties": {"city": {"type": "string"}}}),
    )])
}

#[test]
fn a_rebuild_answers_from_the_new_catalog_and_not_the_old() {
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    let rebuilt = picker.rebuild(other_catalog()).unwrap();

    assert_eq!(rebuilt.tools(), other_catalog().tools());
    assert_eq!(
        rebuilt.config(),
        picker.config(),
        "a rebuild changes the data, not the policy"
    );
    assert_eq!(
        rebuilt
            .resolve("get the weather forecast for a city")
            .unwrap(),
        Outcome::Bind(other_catalog().tools()[0].clone()),
        "the new catalog's tool must be reachable"
    );
    assert_eq!(
        rebuilt.resolve("read a file from disk").unwrap(),
        Outcome::Absent,
        "the old catalog's tools must be gone, not merged in"
    );
    // The engine rebuilt from is immutable: it still answers as it did.
    assert_eq!(
        picker.resolve("read a file from disk").unwrap(),
        Outcome::Bind(tiny_catalog().tools()[0].clone())
    );
}

#[test]
fn a_rebuild_never_reaches_the_weight_loading_path() {
    // Asserted structurally rather than by a clock: the rebuilt engine
    // holds the *same* `Embedder` allocation, so no second checkpoint was
    // parsed. A wall-clock comparison would measure the machine.
    let picker = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    let rebuilt = picker.rebuild(other_catalog()).unwrap();

    assert!(
        Arc::ptr_eq(picker.embedder(), rebuilt.embedder()),
        "a rebuild loaded its own model instead of sharing the loaded one"
    );
}

#[test]
fn two_engines_over_one_encoder_embed_identical_text_identically() {
    let embedder = Arc::new(Embedder::new().unwrap());
    let first =
        ToolPicker::build_with(Arc::clone(&embedder), tiny_catalog(), Config::default()).unwrap();
    let second = ToolPicker::build_with(embedder, tiny_catalog(), Config::default()).unwrap();

    assert_eq!(first.len(), second.len());
    for index in 0..first.len() {
        assert_eq!(
            first.vector(index),
            second.vector(index),
            "row {index} differs between two engines over one encoder"
        );
    }
}

#[test]
fn a_configuration_the_engine_would_reject_is_refused_before_indexing() {
    // `build_with` carries the same check `build` does, so the one
    // indexing path cannot be reached with a configuration `build` would
    // have refused.
    let embedder = Arc::new(Embedder::new().unwrap());
    let error = ToolPicker::build_with(
        embedder,
        tiny_catalog(),
        Config {
            top_k: 0,
            ..Config::default()
        },
    )
    .expect_err("an invalid configuration must not build an engine");
    assert!(matches!(error, Error::EmptyShortlist), "got {error:?}");
}

#[test]
fn building_twice_over_one_catalog_yields_the_same_vectors() {
    let first = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();
    let second = ToolPicker::build(tiny_catalog(), Config::default()).unwrap();

    assert_eq!(first.len(), second.len());
    for index in 0..first.len() {
        assert_eq!(
            first.vector(index),
            second.vector(index),
            "row {index} moved between builds; resolution cannot be deterministic"
        );
    }
}
