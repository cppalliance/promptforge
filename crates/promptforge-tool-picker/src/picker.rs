//! The engine: a catalog embedded once and held ready to be queried.
//!
//! A picker takes ownership of a catalog, embeds every tool in it with a
//! reusable [`Model`], and keeps the descriptors and a validated vector index
//! together for the life of the value. Query results borrow the picker, so a
//! resolution or shortlist never deep-clones a descriptor.
//!
//! [`ToolPicker::build`] is the one-call path that loads a model and indexes a
//! catalog. [`ToolPicker::build_with_model`] indexes several catalogs over one
//! borrowed model, and [`ToolPicker::rebuild`] replaces a picker's catalog while
//! preserving its model and policy. Building over an empty catalog succeeds.

use promptforge_progress::ProgressHandle;

use crate::catalog::{Catalog, ToolDescriptor, ToolId};
use crate::config::Config;
use crate::embed::EMBEDDING_DIMENSIONS;
use crate::error::{BuildError, IndexError, QueryError, SelectionError};
use crate::model::Model;
use crate::policy::{self, Outcome, Shortlist};
use crate::rank::Index;
use crate::selected::{self, NearDuplicates};

#[cfg(test)]
mod tests;

/// A catalog embedded and held in memory, ready to answer needs.
///
/// The expensive work happens once, in [`ToolPicker::build`]. The value is
/// immutable afterwards: a changed catalog calls for a new picker through
/// [`ToolPicker::rebuild`], not a mutation.
#[non_exhaustive]
pub struct ToolPicker {
    /// The tools, in the order the catalog gave them.
    catalog: Catalog,
    /// The thresholds this picker was built with.
    config: Config,
    /// The reusable model, shared with any picker rebuilt from this one.
    model: Model,
    /// The validated vector index: one row per catalog entry.
    index: Index,
}

impl ToolPicker {
    /// Loads the model and indexes a whole catalog.
    ///
    /// The costly call: it loads the compiled-in model and runs one forward
    /// pass per tool. A caller serving several catalogs loads a [`Model`] once
    /// and reaches for [`ToolPicker::build_with_model`] instead.
    ///
    /// # Errors
    /// Returns [`BuildError`] when the model cannot be loaded or the catalog
    /// cannot be indexed.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_tool_picker::{Catalog, Config, ToolDescriptor, ToolId, ToolPicker};
    /// use serde_json::json;
    ///
    /// let catalog = Catalog::new(vec![ToolDescriptor::new(
    ///     ToolId::new("files", "read_file"),
    ///     "Read a file from disk",
    ///     json!({"properties": {"path": {"type": "string"}}}),
    /// )]);
    /// let picker = ToolPicker::build(catalog, Config::default())?;
    /// assert_eq!(picker.len(), 1);
    /// # Ok::<(), promptforge_tool_picker::BuildError>(())
    /// ```
    #[must_use = "a picker that is built and dropped did its costly work for nothing"]
    pub fn build(catalog: Catalog, config: Config) -> Result<Self, BuildError> {
        let model = Model::load()?;
        Ok(Self::build_with_model(&model, catalog, config, None)?)
    }

    /// Indexes a whole catalog with a model that is already loaded.
    ///
    /// The reusable-model path: the cost is one forward pass per tool, and the
    /// model is borrowed rather than reloaded. A `progress` leaf advances one
    /// tool-count step per embedded tool and completes when indexing finishes;
    /// `None` indexes without reporting.
    ///
    /// # Errors
    /// Returns [`IndexError`] when a tool's text cannot be embedded or the
    /// resulting vector layout is malformed.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_tool_picker::{Catalog, Config, Model, ToolPicker};
    ///
    /// let model = Model::load()?;
    /// let first = ToolPicker::build_with_model(&model, Catalog::default(), Config::default(), None)?;
    /// let second = ToolPicker::build_with_model(&model, Catalog::default(), Config::default(), None)?;
    /// assert_eq!(first.len(), second.len());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use = "a picker that is built and dropped did its work for nothing"]
    pub fn build_with_model(
        model: &Model,
        catalog: Catalog,
        config: Config,
        progress: Option<&ProgressHandle>,
    ) -> Result<Self, IndexError> {
        let mut rows = Vec::with_capacity(catalog.len().saturating_mul(EMBEDDING_DIMENSIONS));
        let total = catalog.len() as u64;
        for (embedded, tool) in catalog.as_slice().iter().enumerate() {
            let vector = model
                .embed(&tool.enriched_text())
                .map_err(IndexError::embed)?;
            rows.extend_from_slice(&vector);
            if let Some(handle) = progress {
                handle.set_units(embedded as u64 + 1, total);
            }
        }
        if let Some(handle) = progress {
            handle.complete();
        }
        let index =
            Index::new(rows, EMBEDDING_DIMENSIONS, catalog.len()).map_err(IndexError::layout)?;
        Ok(Self {
            catalog,
            config,
            model: model.clone(),
            index,
        })
    }

    /// A new picker over `catalog`, sharing this picker's model and policy.
    ///
    /// # Errors
    /// Returns [`IndexError`] when a tool's text cannot be embedded.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_tool_picker::{Catalog, Config, ToolPicker};
    ///
    /// let picker = ToolPicker::build(Catalog::default(), Config::default())?;
    /// let rebuilt = picker.rebuild(Catalog::default())?;
    /// assert_eq!(rebuilt.config(), picker.config());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use = "rebuild returns a new picker; the original is left unchanged"]
    pub fn rebuild(&self, catalog: Catalog) -> Result<Self, IndexError> {
        Self::build_with_model(&self.model, catalog, self.config.clone(), None)
    }

    /// Returns the number of tools indexed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.catalog.len()
    }

    /// Returns whether the picker indexes no tools at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.catalog.is_empty()
    }

    /// Iterates the indexed tools, in the order the catalog gave them.
    #[must_use = "iterators are lazy and visit nothing unless consumed"]
    pub fn iter(&self) -> ToolIter<'_> {
        ToolIter {
            inner: self.catalog.as_slice().iter(),
        }
    }

    /// Returns the first indexed tool with the given identity, if any.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.catalog.get(id)
    }

    /// Returns the configuration this picker was built with.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Resolves a need to one of the four borrowing outcomes.
    ///
    /// An abstention is a successful [`Outcome::Absent`]; an `Err` means the
    /// need could not be embedded, so no answer was produced.
    ///
    /// # Errors
    /// Returns [`QueryError`] when the need cannot be embedded. Ranking and
    /// deciding cannot fail.
    pub fn resolve(&self, need: &str) -> Result<Outcome<'_>, QueryError> {
        let query = self.model.embed(need)?;
        let ranked = self.index.top_k(&query, self.config.top_k().get().max(2));
        Ok(policy::decide(
            &ranked,
            self.catalog.as_slice(),
            self.index.vectors(),
            &self.config,
        ))
    }

    /// Returns the best `limit` tools for a need, best first, borrowing them.
    ///
    /// Preserves the solo-candidate exception: a lone leader between the solo
    /// floor and the strict floor is offered, matching what
    /// [`ToolPicker::resolve`] would bind. A `limit` of zero returns an empty
    /// shortlist without embedding the need.
    ///
    /// # Errors
    /// Returns [`QueryError`] when the need cannot be embedded.
    pub fn shortlist(&self, need: &str, limit: usize) -> Result<Shortlist<'_>, QueryError> {
        if limit == 0 {
            return Ok(policy::shortlist(
                &[],
                self.catalog.as_slice(),
                &self.config,
            ));
        }
        let query = self.model.embed(need)?;
        let ranked = self.index.top_k(&query, limit);
        Ok(policy::shortlist(
            &ranked,
            self.catalog.as_slice(),
            &self.config,
        ))
    }

    /// Reports near-duplicate pairs among the selected tool identities.
    ///
    /// Every requested identity must be present. All are validated before any
    /// pair is compared, so an absent identity fails rather than yielding an
    /// incomplete analysis. Repeated identities are idempotent set membership.
    ///
    /// # Errors
    /// Returns [`SelectionError`] naming the first requested identity absent
    /// from this picker's catalog.
    pub fn near_duplicates(&self, ids: &[ToolId]) -> Result<NearDuplicates<'_>, SelectionError> {
        selected::near_duplicates(
            self.catalog.as_slice(),
            self.index.vectors(),
            self.config.duplicate_threshold(),
            ids,
        )
    }

    /// Whether this picker shares its model allocation with `other`.
    #[cfg(test)]
    pub(crate) fn shares_model(&self, other: &ToolPicker) -> bool {
        self.model.shares_encoder(&other.model)
    }

    /// The stored vector row for the tool at `index`, for crate-internal tests.
    #[cfg(test)]
    pub(crate) fn row(&self, index: usize) -> Option<&[f32]> {
        self.index.vectors().row(index)
    }
}

impl std::fmt::Debug for ToolPicker {
    /// Reports the size and shape of the index, never the vectors themselves.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolPicker")
            .field("tools", &self.len())
            .field("dimensions", &EMBEDDING_DIMENSIONS)
            .field("model", &self.model)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl<'a> IntoIterator for &'a ToolPicker {
    type Item = &'a ToolDescriptor;
    type IntoIter = ToolIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A borrowing iterator over a picker's indexed tools.
#[derive(Debug, Clone)]
pub struct ToolIter<'a> {
    /// The backing slice iterator over the picker's descriptors.
    inner: std::slice::Iter<'a, ToolDescriptor>,
}

impl<'a> Iterator for ToolIter<'a> {
    type Item = &'a ToolDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl DoubleEndedIterator for ToolIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back()
    }
}

impl ExactSizeIterator for ToolIter<'_> {}

impl std::iter::FusedIterator for ToolIter<'_> {}
