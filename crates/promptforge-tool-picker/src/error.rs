//! The error type returned by fallible engine operations.
//!
//! Resolution itself is infallible and reports abstention as an outcome, so
//! [`Error`] is the crate's one error type and its variants cover configuration
//! validation - a threshold outside the cosine range, a zero-length shortlist -
//! selected identities absent from the picker, and the embedding backend:
//! loading the compiled-in model, tokenizing, and the forward pass.
//! The type is `#[non_exhaustive]`, so a later failure mode can add a variant
//! without that being a breaking change.
//!
//! No variant carries a dependency's error type. Each one carries a `detail`
//! string instead, so a new release of Candle or of the tokenizer cannot become
//! a breaking change to this crate's public surface.

/// Something that went wrong before the engine could answer a need.
///
/// Every variant names the offending value, because an input rejected without
/// saying which value was wrong is a worse failure than the misbehaviour it
/// prevents.
///
/// The type is `Send + Sync + 'static` and never exposes a dependency's error
/// type. It is `#[non_exhaustive]` at both the enum and the variant level: new
/// failure modes and new fields on an existing one are additive.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A configured threshold fell outside the cosine range `0.0..=1.0`.
    ///
    /// Thresholds are compared against cosine similarities of L2-normalized
    /// vectors, which cannot leave that range, so a value outside it either
    /// admits everything or admits nothing. A NaN lands here too: it compares
    /// false against every bound and would silently disable the check.
    #[non_exhaustive]
    #[error("{field} must be between 0.0 and 1.0, got {value}")]
    ThresholdOutOfRange {
        /// The name of the configuration field, as it is spelled in [`Config`].
        ///
        /// [`Config`]: crate::Config
        field: &'static str,
        /// The rejected value.
        value: f32,
    },

    /// The configured shortlist length was zero.
    ///
    /// A zero-length shortlist has no top-1 to bind and no top-2 to measure a
    /// margin against, so every need would abstain.
    #[error("top_k must be at least 1")]
    EmptyShortlist,

    /// A selected tool identity is not present in the picker's catalog.
    ///
    /// Selected-set analysis is validation, so silently dropping an absent
    /// identity would make an incomplete scope appear safe.
    #[non_exhaustive]
    #[error("tool identity is absent from the picker catalog: {id:?}")]
    ToolNotInCatalog {
        /// The requested identity that the picker could not find.
        id: crate::ToolId,
    },

    /// The embedded model could not be turned into a usable encoder.
    ///
    /// The weights, the tokenizer, and the architecture configuration are all
    /// compiled in, so this is not a missing-file or a bad-download failure -
    /// there is nothing a caller can supply to fix it. It means the embedded
    /// bytes and the code that reads them disagree, which is a build defect.
    #[non_exhaustive]
    #[error("could not load the embedded embedding model: {detail}")]
    ModelLoad {
        /// What went wrong, in the underlying library's own words.
        ///
        /// A string rather than a wrapped source error: the error type of a
        /// private dependency is not part of this crate's public surface, and
        /// exposing one would make its next release a breaking change here.
        detail: String,
    },

    /// The text could not be tokenized.
    #[non_exhaustive]
    #[error("could not tokenize the text to embed: {detail}")]
    Tokenize {
        /// What went wrong, in the tokenizer's own words.
        detail: String,
    },

    /// The forward pass ran but did not yield a usable vector.
    ///
    /// Covers a failure inside the model as well as an output that cannot be
    /// L2-normalized because its length is zero or not finite.
    #[non_exhaustive]
    #[error("could not embed the text: {detail}")]
    Embed {
        /// What went wrong, in the underlying library's own words.
        detail: String,
    },
}

/// The result of a fallible engine operation.
///
/// Defaulting the error parameter lets callers write `Result<Config>` while
/// still being able to name a different error type where one is needed.
pub type Result<T, E = Error> = std::result::Result<T, E>;
