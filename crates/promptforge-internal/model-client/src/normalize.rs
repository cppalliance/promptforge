//! The neutral reply checks every completion passes, whoever built it.
//!
//! A tool call must have a nonblank id and name and object arguments, the
//! calls in one turn must have distinct ids, and a turn with no product is
//! an `EmptyReply`-kind failure. The validating constructors in
//! `client/wire-canned.rs` run these checks, so a completion a wire decoder
//! built and one a Harness built by hand are judged alike. Parsing a
//! provider's wire body lives in `harness-gateway-client`, not here.

use std::collections::HashSet;

use serde_json::Value;

use crate::Result;
use crate::model::{CompletionError, CompletionErrorKind};

/// The empty-reply error for a turn with no product.
pub(crate) fn empty_reply_error() -> CompletionError {
    CompletionError::phrased(CompletionErrorKind::EmptyReply)
}

/// Refuses a blank tool-call id.
pub(crate) fn check_call_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        return Err(CompletionError::malformed("tool call id was blank"));
    }
    Ok(())
}

/// Refuses a blank tool-call name.
pub(crate) fn check_call_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(CompletionError::malformed("tool call name was blank"));
    }
    Ok(())
}

/// Refuses tool-call arguments that are not a JSON object, the shape tools
/// accept.
pub(crate) fn check_call_arguments(arguments: &Value) -> Result<()> {
    if !arguments.is_object() {
        return Err(CompletionError::malformed(
            "tool call arguments were not a JSON object",
        ));
    }
    Ok(())
}

/// Records `id` in `seen`, refusing an id another call in the turn has.
pub(crate) fn check_unique_call_id<'a>(seen: &mut HashSet<&'a str>, id: &'a str) -> Result<()> {
    if !seen.insert(id) {
        return Err(CompletionError::malformed(format!(
            "duplicate tool call id {id:?} within one turn"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "normalize-tests.rs"]
mod tests;
