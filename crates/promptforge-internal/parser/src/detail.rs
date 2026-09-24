//! Operations on the parser's host-visible types that only the engine
//! performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host.

use crate::{Error, ParseError};

/// Unwraps a parse failure into the internal error it classifies, so the
/// engine can map it onto its own internal error type variant for variant.
#[must_use]
pub fn parse_error_into_inner(error: ParseError) -> Error {
    error.into_inner()
}
