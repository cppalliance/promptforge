//! The failure this crate builds for a reply it cannot read.

use std::fmt::Display;

use promptforge::model::{CompletionError, CompletionErrorKind};

/// A `MalformedResponse` failure: the kind's fixed phrase extended with
/// `: ` and `specific`, which this crate's own code wrote. Provider text
/// never goes in `specific`.
pub(crate) fn malformed(specific: impl Display) -> CompletionError {
    let kind = CompletionErrorKind::MalformedResponse;
    CompletionError::new(kind, format!("{}: {specific}", kind.phrase()))
}
