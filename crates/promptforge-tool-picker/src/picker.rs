//! The engine itself: a catalog embedded once and held ready to be queried.
//!
//! [`ToolPicker::build`] is the only way to make one. It takes ownership of a
//! catalog, embeds every tool in it, and keeps the descriptors and their
//! vectors together for the life of the value. Nothing is written to disk and
//! nothing is read from it: an index is a process-lifetime thing, rebuilt from
//! the catalog whenever the catalog changes. A cache keyed on catalog content
//! would have to be invalidated by the same content hash it is keyed on, and
//! embedding a realistic catalog costs less than the correctness risk of
//! serving a stale one.
//!
//! # An empty catalog builds
//!
//! Building over a catalog with no tools succeeds and yields an engine that
//! indexes nothing. It is not an error, because "no tool fits this need" is
//! already an answer this engine gives - a catalog with nothing in it is just
//! the case where that answer is the only one available. Refusing would push a
//! special case onto every caller whose catalog is assembled at run time from
//! whatever servers happened to be reachable, turning an ordinary transient
//! state into a hard failure the caller must branch on. Configuration is
//! rejected when it is nonsense; a catalog is data, and an empty one is
//! perfectly sensible data.
//!
//! # Vectors live in one flat buffer
//!
//! The embeddings are stored as one contiguous buffer holding a single
//! [`EMBEDDING_DIMENSIONS`]-long row per tool, rather than as a vector of
//! vectors: one allocation instead of one per tool, and scoring a need walks
//! the whole buffer in address order, which is the access pattern hardware
//! prefetches. A `Vec<Vec<f32>>` would scatter each tool's 1.5KB across the
//! heap and add a pointer chase per tool for no benefit, since rows are never
//! inserted, removed, or resized after a build.
//!
//! Every stored vector is L2-normalized, because [`Embedder::embed`] returns
//! them that way. The cosine similarity between a need and a tool is therefore
//! the plain dot product of their vectors, with no magnitudes to divide out.

use crate::catalog::{Catalog, ToolDescriptor};
use crate::config::Config;
use crate::embed::{EMBEDDING_DIMENSIONS, Embedder};
use crate::error::Result;

/// A catalog embedded and held in memory, ready to answer needs.
///
/// The expensive work - loading the model and embedding every tool - happens
/// once, in [`ToolPicker::build`]. The value is immutable afterwards: a
/// catalog that has changed calls for a new engine, not a mutated one.
pub struct ToolPicker {
    /// The tools, in the order the catalog gave them.
    catalog: Catalog,
    /// The thresholds and model this engine was built with.
    config: Config,
    /// The encoder, kept so a need is embedded by the same model as the tools.
    embedder: Embedder,
    /// Every tool's vector, end to end, [`EMBEDDING_DIMENSIONS`] apart.
    ///
    /// Row `i` is `vectors[i * EMBEDDING_DIMENSIONS..][..EMBEDDING_DIMENSIONS]`
    /// and describes `catalog.tools()[i]`.
    vectors: Vec<f32>,
}

impl ToolPicker {
    /// Embeds a whole catalog and returns the engine over it.
    ///
    /// The configuration is checked first, so a nonsensical threshold costs
    /// nothing: the failure arrives before the model is loaded and before a
    /// single tool is embedded. Then the model is loaded once and each tool is
    /// embedded from its [`ToolDescriptor::enriched_text`] - the exact text the
    /// configured thresholds were calibrated against.
    ///
    /// The result is deterministic. The same catalog and configuration produce
    /// byte-identical vectors in the same order on every run, because the
    /// enriched text is a pure function of the descriptor and each text is
    /// embedded on its own rather than in a batch.
    ///
    /// This is the costly call in the crate's life cycle: it parses tens of
    /// megabytes of weights and runs one forward pass per tool. An empty
    /// catalog still loads the model, since the model is what a later need
    /// will be embedded with.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Config::validate`] rejects the configuration with;
    /// [`Error::ModelLoad`] if the compiled-in model cannot be loaded; and
    /// [`Error::Tokenize`] or [`Error::Embed`] if a tool's text cannot be
    /// embedded, naming the first failure and abandoning the rest.
    ///
    /// [`Error::ModelLoad`]: crate::Error::ModelLoad
    /// [`Error::Tokenize`]: crate::Error::Tokenize
    /// [`Error::Embed`]: crate::Error::Embed
    pub fn build(catalog: Catalog, config: Config) -> Result<Self> {
        config.validate()?;

        let embedder = Embedder::new()?;
        let mut vectors = Vec::with_capacity(catalog.len() * EMBEDDING_DIMENSIONS);
        for tool in &catalog {
            // `embed` guarantees the length, which is what makes every row of
            // the flat buffer exactly one stride wide.
            vectors.extend_from_slice(&embedder.embed(&tool.enriched_text())?);
        }

        Ok(Self {
            catalog,
            config,
            embedder,
            vectors,
        })
    }

    /// The number of tools indexed.
    ///
    /// Always the length of the catalog that was built: every tool is embedded
    /// or the build fails, so there is no partial index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.catalog.len()
    }

    /// Whether the engine indexes no tools at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.catalog.is_empty()
    }

    /// The indexed tools, in the order the catalog gave them.
    ///
    /// The order is the one the caller supplied and is the order every other
    /// per-tool accessor is indexed by.
    #[must_use]
    pub fn tools(&self) -> &[ToolDescriptor] {
        self.catalog.tools()
    }

    /// The configuration this engine was built with.
    ///
    /// It is the validated copy the engine will decide by, which is worth
    /// reading back when the caller assembled it from defaults and overrides.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The stored vector for the tool at `index` in [`ToolPicker::tools`].
    ///
    /// The slice is [`EMBEDDING_DIMENSIONS`] long and unit length, so its dot
    /// product with any other such vector is a cosine similarity. Returns
    /// `None` when `index` is past the end.
    #[must_use]
    pub fn vector(&self, index: usize) -> Option<&[f32]> {
        let start = index.checked_mul(EMBEDDING_DIMENSIONS)?;
        let end = start.checked_add(EMBEDDING_DIMENSIONS)?;
        self.vectors.get(start..end)
    }
}

impl std::fmt::Debug for ToolPicker {
    /// Reports the size and shape of the index, never the vectors themselves.
    ///
    /// Printing hundreds of thousands of floats would be useless to a reader
    /// and ruinous in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolPicker")
            .field("tools", &self.len())
            .field("dimensions", &EMBEDDING_DIMENSIONS)
            .field("embedder", &self.embedder)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::ToolPicker;
    use crate::catalog::{Catalog, ToolDescriptor, ToolId};
    use crate::config::Config;
    use crate::embed::EMBEDDING_DIMENSIONS;
    use crate::error::Error;
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
    fn a_duplicate_threshold_below_the_floor_is_rejected_as_validate_rejects_it() {
        let (built, validated) = rejections(&Config {
            similarity_floor: 0.9,
            duplicate_threshold: 0.85,
            ..Config::default()
        });
        assert!(
            matches!(
                (&built, &validated),
                (
                    Error::DuplicateThresholdBelowFloor { .. },
                    Error::DuplicateThresholdBelowFloor { .. }
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
}
