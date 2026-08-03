//! The error type returned by fallible engine operations.
//!
//! Resolution itself is infallible and reports abstention as an outcome, so
//! [`Error`] is the crate's one error type and its variants cover configuration
//! validation: a threshold outside the cosine range, a zero-length shortlist,
//! and a duplicate threshold below the similarity floor. The type is
//! `#[non_exhaustive]`, so other failure modes - loading the embedded model,
//! tokenizing, running the forward pass - can add variants without that being a
//! breaking change.

/// Something that went wrong before the engine could answer a need.
///
/// Every variant names the offending value, because a configuration rejected
/// without saying which field was wrong is a worse failure than the misbehaviour
/// it prevents.
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
        /// [`Config`]: crate::config::Config
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

    /// The duplicate threshold sat below the similarity floor.
    ///
    /// Two tools are twins only if they are both plausible and nearly
    /// indistinguishable, so the duplicate threshold is the stricter of the
    /// two. Below the floor it would flag pairs the floor has already rejected.
    #[non_exhaustive]
    #[error(
        "duplicate_threshold {duplicate_threshold} must be at least similarity_floor {similarity_floor}"
    )]
    DuplicateThresholdBelowFloor {
        /// The configured similarity floor.
        similarity_floor: f32,
        /// The configured duplicate threshold, which fell below it.
        duplicate_threshold: f32,
    },
}

/// The result of a fallible engine operation.
///
/// Defaulting the error parameter lets callers write `Result<Config>` while
/// still being able to name a different error type where one is needed.
pub type Result<T, E = Error> = std::result::Result<T, E>;
