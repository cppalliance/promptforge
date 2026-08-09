//! Narrow opaque errors for each fallible operation.

use std::error::Error as StdError;

use crate::{ConfigField, ToolId};

type Source = Box<dyn StdError + Send + Sync + 'static>;

macro_rules! opaque_error {
    ($name:ident, $repr:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, thiserror::Error)]
        #[error(transparent)]
        pub struct $name(pub(crate) $repr);
    };
}

opaque_error!(
    ConfigError,
    ConfigErrorRepr,
    "An invalid configuration override."
);
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigErrorRepr {
    #[error("threshold outside supported range")]
    Threshold { field: ConfigField, value: f32 },
    #[error("zero result-group limit")]
    ZeroTopK,
}
impl ConfigError {
    /// Returns the rejected field.
    #[must_use]
    pub fn field(&self) -> ConfigField {
        match self.0 {
            ConfigErrorRepr::Threshold { field, .. } => field,
            ConfigErrorRepr::ZeroTopK => ConfigField::TopK,
        }
    }
}

opaque_error!(
    ModelLoadError,
    ModelLoadErrorRepr,
    "A failure while loading the embedded model."
);
#[derive(Debug, thiserror::Error)]
pub(crate) enum ModelLoadErrorRepr {
    #[error("embedded model configuration")]
    Config(#[source] Source),
    #[error("embedded model tokenizer")]
    Tokenizer(#[source] Source),
    #[error("embedded model weights")]
    Weights(#[source] Source),
    #[error("embedded model architecture")]
    Architecture(#[source] Source),
    #[error("embedded model dimension mismatch")]
    Dimensions,
}

opaque_error!(
    IndexError,
    IndexErrorRepr,
    "A failure while indexing a catalog."
);
#[derive(Debug, thiserror::Error)]
pub(crate) enum IndexErrorRepr {
    #[error("catalog tokenization")]
    Tokenization(#[source] Source),
    #[error("catalog inference")]
    Inference(#[source] Source),
    #[error("invalid catalog embedding")]
    InvalidEmbedding,
}

opaque_error!(
    BuildError,
    BuildErrorRepr,
    "A failure while loading a model or indexing a catalog."
);
#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildErrorRepr {
    #[error("model loading")]
    Model(#[source] ModelLoadError),
    #[error("catalog indexing")]
    Index(#[source] IndexError),
}
impl From<ModelLoadError> for BuildError {
    fn from(value: ModelLoadError) -> Self {
        Self(BuildErrorRepr::Model(value))
    }
}
impl From<IndexError> for BuildError {
    fn from(value: IndexError) -> Self {
        Self(BuildErrorRepr::Index(value))
    }
}

/// Classifies a query failure without exposing backend types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueryErrorKind {
    /// Tokenization failed.
    Tokenization,
    /// Model inference failed.
    Inference,
    /// The model returned an unusable vector.
    InvalidEmbedding,
}

opaque_error!(
    QueryError,
    QueryErrorRepr,
    "A failure while resolving or shortlisting a need."
);
#[derive(Debug, thiserror::Error)]
pub(crate) enum QueryErrorRepr {
    #[error("query tokenization")]
    Tokenization(#[source] Source),
    #[error("query inference")]
    Inference(#[source] Source),
    #[error("invalid query embedding")]
    InvalidEmbedding,
}
impl QueryError {
    /// Returns the stable failure classification.
    #[must_use]
    pub fn kind(&self) -> QueryErrorKind {
        match self.0 {
            QueryErrorRepr::Tokenization(_) => QueryErrorKind::Tokenization,
            QueryErrorRepr::Inference(_) => QueryErrorKind::Inference,
            QueryErrorRepr::InvalidEmbedding => QueryErrorKind::InvalidEmbedding,
        }
    }
}

/// A selected identity absent from the picker catalog.
#[derive(Debug, thiserror::Error)]
#[error("tool absent from selected catalog scope")]
pub struct SelectionError {
    pub(crate) missing: ToolId,
}
impl SelectionError {
    /// Returns the first missing requested identity.
    #[must_use]
    pub fn missing_id(&self) -> &ToolId {
        &self.missing
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    const fn assert_error<T: Send + Sync + 'static>() {}

    #[test]
    fn public_errors_have_required_auto_traits_and_lowercase_messages() {
        assert_error::<ConfigError>();
        assert_error::<ModelLoadError>();
        assert_error::<IndexError>();
        assert_error::<BuildError>();
        assert_error::<QueryError>();
        assert_error::<SelectionError>();

        let config = ConfigError(ConfigErrorRepr::ZeroTopK);
        assert_eq!(config.to_string(), "zero result-group limit");
        let source = ModelLoadError(ModelLoadErrorRepr::Config(Box::new(std::io::Error::other(
            "bad",
        ))));
        assert!(source.source().is_some());
    }
}
