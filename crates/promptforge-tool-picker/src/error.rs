//! The narrow, opaque errors returned by fallible resolver operations.
//!
//! There is no crate-wide error enum. Each fallible operation names the unit of
//! fallibility it can produce: [`ModelLoadError`] for loading the compiled-in
//! model, [`IndexError`] for indexing a catalog, [`BuildError`] for the one-call
//! build that does both, [`QueryError`] for embedding a need, and
//! [`SelectionError`] for validating a selected scope. [`crate::ConfigError`]
//! covers configuration construction.
//!
//! Every error is an opaque wrapper over a private representation that retains
//! the dependency error behind [`std::error::Error::source`]. No Candle,
//! tokenizer, serde, or safetensors type appears in a public signature.

use crate::catalog::ToolId;

/// A boxed dependency error retained as an opaque source.
type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The embedded model could not be turned into a usable encoder.
///
/// Every cause is a build defect - the weights, tokenizer, and architecture are
/// all compiled in, so there is nothing a caller can supply to fix it. The
/// underlying dependency error is retained as the source.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ModelLoadError(#[from] ModelLoadRepr);

/// The private, operation-specific representation behind [`ModelLoadError`].
#[derive(Debug, thiserror::Error)]
enum ModelLoadRepr {
    /// The embedded `config.json` is not a BERT configuration.
    #[error("embedded model configuration")]
    Config(#[source] BoxError),
    /// The embedded model's hidden size disagrees with the compiled dimension.
    #[error("embedded model dimension mismatch: hidden size {got}, expected {expected}")]
    Dimensions {
        /// The hidden size the embedded checkpoint declared.
        got: usize,
        /// The dimension this crate was compiled for.
        expected: usize,
    },
    /// The embedded weights do not carry the pinned repository and revision.
    #[error("embedded model provenance: {0}")]
    Provenance(String),
    /// The embedded `tokenizer.json` could not be parsed.
    #[error("embedded tokenizer")]
    Tokenizer(#[source] BoxError),
    /// Truncation at the model's sequence length was rejected.
    #[error("tokenizer truncation setup")]
    Truncation(#[source] BoxError),
    /// The embedded weights are not a readable safetensors blob.
    #[error("embedded model weights")]
    Weights(#[source] BoxError),
    /// The embedded weights do not fit the BERT architecture.
    #[error("embedded model architecture")]
    Model(#[source] BoxError),
}

impl ModelLoadError {
    pub(crate) fn config(source: impl Into<BoxError>) -> Self {
        Self(ModelLoadRepr::Config(source.into()))
    }

    pub(crate) fn dimensions(got: usize, expected: usize) -> Self {
        Self(ModelLoadRepr::Dimensions { got, expected })
    }

    pub(crate) fn provenance(detail: impl Into<String>) -> Self {
        Self(ModelLoadRepr::Provenance(detail.into()))
    }

    pub(crate) fn tokenizer(source: impl Into<BoxError>) -> Self {
        Self(ModelLoadRepr::Tokenizer(source.into()))
    }

    pub(crate) fn truncation(source: impl Into<BoxError>) -> Self {
        Self(ModelLoadRepr::Truncation(source.into()))
    }

    pub(crate) fn weights(source: impl Into<BoxError>) -> Self {
        Self(ModelLoadRepr::Weights(source.into()))
    }

    pub(crate) fn model(source: impl Into<BoxError>) -> Self {
        Self(ModelLoadRepr::Model(source.into()))
    }
}

/// A need could not be embedded, so no answer was produced.
///
/// A query error is not an abstention: [`crate::Outcome::Absent`] is a
/// successful policy answer, while this means the engine could not run. Use
/// [`QueryError::kind`] to classify the failure.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct QueryError(#[from] QueryRepr);

/// The private, operation-specific representation behind [`QueryError`].
#[derive(Debug, thiserror::Error)]
enum QueryRepr {
    /// The need text could not be tokenized.
    #[error("need tokenization")]
    Tokenization(#[source] BoxError),
    /// The forward pass over the need failed.
    #[error("need inference")]
    Inference(#[source] BoxError),
    /// The forward pass produced a vector that cannot be used.
    #[error("invalid need embedding: {0}")]
    InvalidEmbedding(String),
}

/// A stable classification of why a need could not be embedded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueryErrorKind {
    /// The need text could not be tokenized.
    Tokenization,
    /// The model's forward pass over the need failed.
    Inference,
    /// The produced vector could not be normalized into a usable embedding.
    InvalidEmbedding,
}

impl QueryError {
    /// Returns the stable classification of this failure.
    #[must_use]
    pub fn kind(&self) -> QueryErrorKind {
        match &self.0 {
            QueryRepr::Tokenization(_) => QueryErrorKind::Tokenization,
            QueryRepr::Inference(_) => QueryErrorKind::Inference,
            QueryRepr::InvalidEmbedding(_) => QueryErrorKind::InvalidEmbedding,
        }
    }

    pub(crate) fn tokenization(source: impl Into<BoxError>) -> Self {
        Self(QueryRepr::Tokenization(source.into()))
    }

    pub(crate) fn inference(source: impl Into<BoxError>) -> Self {
        Self(QueryRepr::Inference(source.into()))
    }

    pub(crate) fn invalid_embedding(detail: impl Into<String>) -> Self {
        Self(QueryRepr::InvalidEmbedding(detail.into()))
    }
}

/// A catalog could not be indexed into a picker.
///
/// Either a tool's text could not be embedded, or the private vector layout
/// broke an internal invariant. The underlying cause is retained as the source.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct IndexError(#[from] IndexRepr);

/// The private, operation-specific representation behind [`IndexError`].
#[derive(Debug, thiserror::Error)]
enum IndexRepr {
    /// A tool's enriched text could not be embedded.
    #[error("catalog embedding")]
    Embed(#[source] QueryError),
    /// The private vector layout did not describe one row per catalog entry.
    #[error("catalog vector layout: {0}")]
    Layout(String),
}

impl IndexError {
    pub(crate) fn embed(source: QueryError) -> Self {
        Self(IndexRepr::Embed(source))
    }

    pub(crate) fn layout(detail: impl Into<String>) -> Self {
        Self(IndexRepr::Layout(detail.into()))
    }
}

/// A picker could not be built, either loading the model or indexing.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct BuildError(#[from] BuildRepr);

/// The private, operation-specific representation behind [`BuildError`].
#[derive(Debug, thiserror::Error)]
enum BuildRepr {
    /// The compiled-in model could not be loaded.
    #[error(transparent)]
    Load(ModelLoadError),
    /// The catalog could not be indexed.
    #[error(transparent)]
    Index(IndexError),
}

impl From<ModelLoadError> for BuildError {
    fn from(error: ModelLoadError) -> Self {
        Self(BuildRepr::Load(error))
    }
}

impl From<IndexError> for BuildError {
    fn from(error: IndexError) -> Self {
        Self(BuildRepr::Index(error))
    }
}

/// A selected tool identity was not present in the picker's catalog.
///
/// Selected-scope analysis is validation, so an absent identity fails rather
/// than being silently dropped. The first missing identity is reported.
#[derive(Debug, thiserror::Error)]
#[error("selected tool identity absent from the picker catalog: {missing:?}")]
pub struct SelectionError {
    /// The first requested identity the picker could not find.
    missing: ToolId,
}

impl SelectionError {
    pub(crate) fn new(missing: ToolId) -> Self {
        Self { missing }
    }

    /// Returns the first requested identity that was absent from the catalog.
    #[must_use]
    pub fn missing_id(&self) -> &ToolId {
        &self.missing
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BuildError, IndexError, ModelLoadError, QueryError, QueryErrorKind, SelectionError,
    };
    use crate::catalog::ToolId;

    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    #[test]
    fn every_public_error_is_send_sync_static() {
        assert_send_sync_static::<ModelLoadError>();
        assert_send_sync_static::<IndexError>();
        assert_send_sync_static::<BuildError>();
        assert_send_sync_static::<QueryError>();
        assert_send_sync_static::<SelectionError>();
        assert_send_sync_static::<QueryErrorKind>();
    }

    #[test]
    fn a_query_error_classifies_and_displays_as_a_lowercase_noun_phrase() {
        let error = QueryError::invalid_embedding("length zero");
        assert_eq!(error.kind(), QueryErrorKind::InvalidEmbedding);
        assert_eq!(error.to_string(), "invalid need embedding: length zero");
    }

    #[test]
    fn a_build_error_wraps_an_index_error_transparently() {
        let build = BuildError::from(IndexError::layout("count mismatch"));
        assert_eq!(build.to_string(), "catalog vector layout: count mismatch");
    }

    #[test]
    fn an_index_embed_error_retains_its_query_source() {
        let index = IndexError::embed(QueryError::invalid_embedding("length zero"));
        assert!(
            std::error::Error::source(&index).is_some(),
            "an embedding failure retains its query-error source"
        );
        let build = BuildError::from(index);
        assert!(std::error::Error::source(&build).is_some());
    }

    #[test]
    fn a_selection_error_reports_the_missing_identity() {
        let missing = ToolId::new("files", "read_file");
        let error = SelectionError::new(missing.clone());
        assert_eq!(error.missing_id(), &missing);
    }
}
