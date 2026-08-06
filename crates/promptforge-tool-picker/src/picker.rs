//! The engine itself: a catalog embedded once and held ready to be queried.
//!
//! An engine takes ownership of a catalog, embeds every tool in it, and keeps
//! the descriptors and their vectors together for the life of the value.
//! Nothing is written to disk and nothing is read from it: an index is a
//! process-lifetime thing, rebuilt from the catalog whenever the catalog
//! changes. A cache keyed on catalog content would have to be invalidated by
//! the same content hash it is keyed on, and embedding a realistic catalog
//! costs less than the correctness risk of serving a stale one.
//!
//! # One encoder, many engines
//!
//! [`ToolPicker::build_with`] is the constructor everything else is written in
//! terms of: it takes an already-loaded encoder behind an `Arc` and pays only
//! one forward pass per tool. [`ToolPicker::build`] loads an encoder and calls
//! it, and [`ToolPicker::rebuild`] calls it with this engine's own encoder and
//! configuration. There is therefore one indexing path and one policy, and a
//! caller whose catalog changes - a watched directory, a reconnected server -
//! pays the weights once for the life of the process rather than once per
//! catalog.
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

use std::sync::Arc;

use crate::catalog::{Catalog, ToolDescriptor, ToolId};
use crate::config::Config;
use crate::embed::{EMBEDDING_DIMENSIONS, Embedder};
use crate::error::{Error, Result};
use crate::policy::{self, Outcome};
use crate::rank::{self, Candidate, Vectors};

#[cfg(test)]
mod tests;

/// A selected pair whose stored tool vectors meet the duplicate threshold.
///
/// The pair is reported in catalog order. Its similarity is the cosine between
/// the tools' stored vectors, independent of any capability need or query.
#[derive(Debug, Clone, PartialEq)]
pub struct NearDuplicate {
    /// The earlier tool in catalog order.
    pub first: ToolDescriptor,
    /// The later tool in catalog order.
    pub second: ToolDescriptor,
    /// The cosine similarity between the tools' stored vectors.
    pub similarity: f32,
}

/// A catalog embedded and held in memory, ready to answer needs.
///
/// The expensive work - loading the model and embedding every tool - happens
/// once, in [`ToolPicker::build`]. The value is immutable afterwards: a
/// catalog that has changed calls for a new engine, not a mutated one, and
/// [`ToolPicker::rebuild`] makes that new engine over the encoder this one
/// already holds.
pub struct ToolPicker {
    /// The tools, in the order the catalog gave them.
    catalog: Catalog,
    /// The thresholds and model this engine was built with.
    config: Config,
    /// The encoder, kept so a need is embedded by the same model as the tools.
    ///
    /// Shared rather than owned outright, so several engines over different
    /// catalogs can hold one set of weights.
    embedder: Arc<Embedder>,
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
    /// will be embedded with. A caller building several engines, or rebuilding
    /// one over a catalog that changed, pays the weights once by reaching for
    /// [`ToolPicker::build_with`] or [`ToolPicker::rebuild`] instead.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Config::validate`] rejects the configuration with;
    /// [`Error::ModelLoad`] if the compiled-in model cannot be loaded; and
    /// [`Error::Tokenize`] or [`Error::Embed`] if a tool's text cannot be
    /// embedded, naming the first failure and abandoning the rest.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use promptforge_tool_picker::{Catalog, Config, ToolDescriptor, ToolId, ToolPicker};
    /// use serde_json::json;
    ///
    /// let catalog = Catalog::new(vec![ToolDescriptor::new(
    ///     ToolId::new("files", "read_file"),
    ///     "Read a file from disk",
    ///     json!({"properties": {"path": {"type": "string"}}}),
    /// )]);
    ///
    /// let picker = ToolPicker::build(catalog, Config::default())?;
    /// assert_eq!(picker.len(), 1);
    /// # Ok::<(), promptforge_tool_picker::Error>(())
    /// ```
    ///
    /// [`Error::ModelLoad`]: crate::Error::ModelLoad
    /// [`Error::Tokenize`]: crate::Error::Tokenize
    /// [`Error::Embed`]: crate::Error::Embed
    pub fn build(catalog: Catalog, config: Config) -> Result<Self> {
        // Validated before the model is loaded, which is the whole reason this
        // check is not left to `build_with` alone: a rejected threshold must
        // not cost tens of megabytes of weights first.
        config.validate()?;
        Self::build_with(Arc::new(Embedder::new()?), catalog, config)
    }

    /// Embeds a whole catalog with an encoder that is already loaded.
    ///
    /// The one indexing path in the crate: [`ToolPicker::build`] loads an
    /// encoder and calls this, and [`ToolPicker::rebuild`] calls it with an
    /// existing engine's encoder, so every engine is indexed under the same
    /// policy and the same enriched text.
    ///
    /// This is the constructor to reach for when one process serves several
    /// catalogs, or one catalog that changes. Loading the model is what makes
    /// [`ToolPicker::build`] costly; here the cost is one forward pass per
    /// tool, and the encoder is shared rather than reloaded. Two engines built
    /// from one encoder embed identical text to identical vectors, because the
    /// weights are the same object and each text is embedded on its own.
    ///
    /// # Errors
    ///
    /// Returns whatever [`Config::validate`] rejects the configuration with,
    /// and [`Error::Tokenize`] or [`Error::Embed`] if a tool's text cannot be
    /// embedded, naming the first failure and abandoning the rest. It cannot
    /// return [`Error::ModelLoad`]: the caller has already loaded the model.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::Arc;
    ///
    /// use promptforge_tool_picker::{Catalog, Config, Embedder, ToolPicker};
    ///
    /// // One set of weights, an engine per catalog.
    /// let embedder = Arc::new(Embedder::new()?);
    /// let first = ToolPicker::build_with(Arc::clone(&embedder), Catalog::default(), Config::default())?;
    /// let second = ToolPicker::build_with(embedder, Catalog::default(), Config::default())?;
    /// assert_eq!(first.len(), second.len());
    /// # Ok::<(), promptforge_tool_picker::Error>(())
    /// ```
    ///
    /// [`Error::ModelLoad`]: crate::Error::ModelLoad
    /// [`Error::Tokenize`]: crate::Error::Tokenize
    /// [`Error::Embed`]: crate::Error::Embed
    pub fn build_with(embedder: Arc<Embedder>, catalog: Catalog, config: Config) -> Result<Self> {
        config.validate()?;

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

    /// A new engine over `catalog`, sharing this engine's encoder and configuration.
    ///
    /// The answer to a catalog that changed. This engine is untouched - it is
    /// immutable, and a run using it finishes under the catalog it started
    /// with - and the returned one indexes the new catalog for the cost of one
    /// forward pass per tool. No weights are loaded, so a rebuild costs what
    /// the catalog costs and nothing more.
    ///
    /// The configuration carries over, because a rebuild is a change of data
    /// and not of policy; a caller changing a threshold builds with
    /// [`ToolPicker::build_with`] instead, handing it
    /// [`ToolPicker::embedder`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tokenize`] or [`Error::Embed`] if a tool's text cannot
    /// be embedded. The configuration was validated when this engine was
    /// built, so it cannot be rejected here.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use promptforge_tool_picker::{Catalog, Config, ToolPicker};
    ///
    /// let picker = ToolPicker::build(Catalog::default(), Config::default())?;
    /// let rebuilt = picker.rebuild(Catalog::default())?;
    /// assert_eq!(rebuilt.config(), picker.config());
    /// # Ok::<(), promptforge_tool_picker::Error>(())
    /// ```
    ///
    /// [`Error::Tokenize`]: crate::Error::Tokenize
    /// [`Error::Embed`]: crate::Error::Embed
    pub fn rebuild(&self, catalog: Catalog) -> Result<Self> {
        Self::build_with(Arc::clone(&self.embedder), catalog, self.config.clone())
    }

    /// The loaded encoder behind this engine, for building another over it.
    ///
    /// Handed to [`ToolPicker::build_with`] to index a second catalog, or a
    /// changed one under a changed configuration, without paying for the
    /// weights again. It is also the encoder every need this engine answers is
    /// embedded by, so a caller embedding text itself compares like with like.
    #[must_use]
    pub fn embedder(&self) -> &Arc<Embedder> {
        &self.embedder
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

    /// Embeds a need and returns the best `k` tools for it, best first.
    ///
    /// The need takes the same path through the same encoder the tools took,
    /// so its vector is unit length too and each score is a cosine similarity.
    /// Ordering, ties, and what a `k` larger than the catalog returns are
    /// [`rank::top_k`]'s contract.
    ///
    /// This ranks and decides nothing: every candidate is returned on its
    /// score alone, with no threshold applied and no candidate withheld.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tokenize`] or [`Error::Embed`] if the need cannot be
    /// embedded. Ranking itself cannot fail.
    ///
    /// [`Error::Tokenize`]: crate::Error::Tokenize
    /// [`Error::Embed`]: crate::Error::Embed
    pub(crate) fn rank(&self, need: &str, k: usize) -> Result<Vec<Candidate>> {
        let query = self.embedder.embed(need)?;
        Ok(rank::top_k(&query, &self.vectors, k))
    }

    /// Resolves a need to one of the four outcomes.
    ///
    /// This is the engine's answer to "which tool does this need mean". The
    /// need is embedded by the model the tools were embedded with, ranked
    /// against the whole index, and judged against the configured thresholds;
    /// what the four outcomes mean, in what order the thresholds apply, and
    /// where each boundary falls is [`Outcome`]'s contract.
    ///
    /// At least two candidates are ranked however short
    /// [`Config::top_k`](crate::Config::top_k) is. Every ambiguity the policy
    /// can report is a statement about the leader and a runner-up, so a
    /// ranking of one would leave the duplicate and near-tie checks nothing to
    /// look at and turn a `top_k` of one - which is a valid configuration -
    /// into a silent binding of whatever came first.
    ///
    /// # An abstention is an answer, not a failure
    ///
    /// A need that matches nothing resolves to `Ok(`[`Outcome::Absent`]`)`.
    /// The `Err` arm means something else entirely: the engine could not run -
    /// the need could not be tokenized or the forward pass failed - so there
    /// is no answer at all rather than an answer of "nothing". A caller that
    /// treats an error as "no tool matched" is discarding a real fault, and
    /// one that treats an abstention as an error is treating the engine's
    /// most careful answer as a bug.
    ///
    /// # Determinism
    ///
    /// The same engine and the same need always resolve to the same outcome,
    /// as do two engines built from the same catalog and configuration.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tokenize`] or [`Error::Embed`] if the need cannot be
    /// embedded. Ranking and deciding cannot fail.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use promptforge_tool_picker::{
    ///     Catalog, Config, Outcome, ToolDescriptor, ToolId, ToolPicker,
    /// };
    /// use serde_json::json;
    ///
    /// # let catalog = Catalog::new(vec![ToolDescriptor::new(
    /// #     ToolId::new("files", "read_file"),
    /// #     "Read a file from disk",
    /// #     json!({"properties": {"path": {"type": "string"}}}),
    /// # )]);
    /// let picker = ToolPicker::build(catalog, Config::default())?;
    ///
    /// match picker.resolve("read a file from disk")? {
    ///     Outcome::Bind(tool) => println!("call {}", tool.name()),
    ///     Outcome::Duplicate(twins) => println!("{} publishes twins", twins[0].server()),
    ///     Outcome::Ambiguous(candidates) => println!("{} tools fit", candidates.len()),
    ///     Outcome::Absent => println!("no tool covers this need"),
    /// }
    /// # Ok::<(), promptforge_tool_picker::Error>(())
    /// ```
    ///
    /// [`Error::Tokenize`]: crate::Error::Tokenize
    /// [`Error::Embed`]: crate::Error::Embed
    pub fn resolve(&self, need: &str) -> Result<Outcome> {
        let ranked = self.rank(need, self.config.top_k.max(2))?;
        Ok(policy::decide(
            &ranked,
            self.tools(),
            Vectors::new(&self.vectors, EMBEDDING_DIMENSIONS),
            &self.config,
        ))
    }

    /// The best `k` tools for a need, best first, for a caller that will choose.
    ///
    /// Where [`ToolPicker::resolve`] decides, this reports. It is the entry
    /// point for a caller that has context the engine does not - a
    /// conversation, a user to ask, a policy of its own - and wants the
    /// candidates rather than a verdict. The order is the one
    /// [`ToolPicker::resolve`] decides under: score descending, with an exact
    /// tie broken by the behavioural hints and then by catalog position.
    ///
    /// # Only candidates above the floor are offered
    ///
    /// A candidate scoring below [`Config::similarity_floor`] is left out, so
    /// a need nothing matches yields an empty list rather than the least bad
    /// of the mismatches. The alternative - returning the raw top `k` whatever
    /// they scored - would make the two public entry points contradict each
    /// other, with [`ToolPicker::resolve`] abstaining on a need while this
    /// method offered three tools for it, and a caller feeding those
    /// candidates into a prompt would be offering tools the engine had already
    /// judged irrelevant.
    ///
    /// A caller who does want to see near-misses lowers
    /// [`Config::similarity_floor`], which is exactly the dial that says how
    /// good a match has to be to count as one. That is a deliberate choice
    /// stated in the configuration rather than a silent property of which
    /// method was called.
    ///
    /// # `k` is authoritative
    ///
    /// `k` is the number of candidates asked for, and it is not clamped
    /// against [`Config::top_k`]. The two describe different things:
    /// `top_k` is how long a shortlist [`ToolPicker::resolve`] reports when it
    /// cannot separate the leaders, while `k` is this caller's own request, on
    /// this call. A `k` beyond the catalog returns everything that clears the
    /// floor and never a padded list, and a `k` of zero returns nothing.
    ///
    /// Fewer than `k` tools may come back for either reason: the catalog was
    /// shorter, or the floor removed some. The list is never longer than `k`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Tokenize`] or [`Error::Embed`] if the need cannot be
    /// embedded. Ranking cannot fail.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use promptforge_tool_picker::{Catalog, Config, ToolDescriptor, ToolId, ToolPicker};
    /// use serde_json::json;
    ///
    /// # let catalog = Catalog::new(vec![ToolDescriptor::new(
    /// #     ToolId::new("files", "read_file"),
    /// #     "Read a file from disk",
    /// #     json!({"properties": {"path": {"type": "string"}}}),
    /// # )]);
    /// let picker = ToolPicker::build(catalog, Config::default())?;
    ///
    /// // Never longer than `k`, and shorter when the floor removed a
    /// // candidate or the catalog had fewer to offer.
    /// let candidates = picker.shortlist("read a file from disk", 3)?;
    /// assert!(candidates.len() <= 3);
    /// # Ok::<(), promptforge_tool_picker::Error>(())
    /// ```
    ///
    /// [`Config::similarity_floor`]: crate::Config::similarity_floor
    /// [`Config::top_k`]: crate::Config::top_k
    /// [`Error::Tokenize`]: crate::Error::Tokenize
    /// [`Error::Embed`]: crate::Error::Embed
    pub fn shortlist(&self, need: &str, k: usize) -> Result<Vec<ToolDescriptor>> {
        let ranked = self.rank(need, k)?;
        Ok(policy::shortlist(&ranked, self.tools(), &self.config))
    }

    /// Reports near-duplicate pairs among the selected tool identities.
    ///
    /// Each selected catalog entry is compared with every later selected entry
    /// using the vectors already stored by this picker. A pair is returned when
    /// its cosine similarity is greater than or equal to
    /// [`Config::duplicate_threshold`]. Server boundaries do not affect this
    /// analysis.
    ///
    /// Every requested identity must be present in the picker. All identities
    /// are validated before any pair is compared, so an absent identity returns
    /// an error rather than an incomplete analysis. Repeating an identity in
    /// `ids` is idempotent set membership: it does not repeat entries or compare
    /// a tool with itself. Results are ordered by the first tool's catalog
    /// position and then the second's, independently of the order of `ids`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ToolNotInCatalog`] naming the first requested identity
    /// that is absent from this picker's catalog.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use promptforge_tool_picker::{Catalog, Config, ToolId, ToolPicker};
    ///
    /// let picker = ToolPicker::build(Catalog::default(), Config::default())?;
    /// let pairs = picker.near_duplicates(&[])?;
    /// assert!(pairs.is_empty());
    /// # Ok::<(), promptforge_tool_picker::Error>(())
    /// ```
    pub fn near_duplicates(&self, ids: &[ToolId]) -> Result<Vec<NearDuplicate>> {
        near_duplicates(
            self.tools(),
            Vectors::new(&self.vectors, EMBEDDING_DIMENSIONS),
            self.config.duplicate_threshold,
            ids,
        )
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

/// Finds selected pairs meeting `threshold`, preserving catalog pair order.
fn near_duplicates(
    tools: &[ToolDescriptor],
    vectors: Vectors<'_>,
    threshold: f32,
    ids: &[ToolId],
) -> Result<Vec<NearDuplicate>> {
    for id in ids {
        if !tools.iter().any(|tool| tool.id == *id) {
            return Err(Error::ToolNotInCatalog { id: id.clone() });
        }
    }

    let mut pairs = Vec::new();
    for (first_index, first) in tools.iter().enumerate() {
        if !ids.contains(&first.id) {
            continue;
        }
        for (second_index, second) in tools.iter().enumerate().skip(first_index + 1) {
            if !ids.contains(&second.id) {
                continue;
            }
            let Some(similarity) = vectors.similarity(first_index, second_index) else {
                continue;
            };
            if similarity >= threshold {
                pairs.push(NearDuplicate {
                    first: first.clone(),
                    second: second.clone(),
                    similarity,
                });
            }
        }
    }
    Ok(pairs)
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
