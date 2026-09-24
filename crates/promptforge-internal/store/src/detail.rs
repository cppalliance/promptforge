//! Operations on [`StoreError`] that only the engine performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host: a host reads a store failure through
//! [`StoreError::kind`] and its accessors, and never builds one.

use crate::StoreError;

/// Builds [`StoreError::InvalidRange`] for `path` with `reason`.
///
/// The Lua host refuses an `end` without a `start` with the same
/// `InvalidRange` a zero bound triggers, but cannot construct the
/// `#[non_exhaustive]` variant directly.
#[must_use]
pub fn store_error_invalid_range(path: &str, reason: &'static str) -> StoreError {
    StoreError::InvalidRange {
        path: path.to_owned(),
        reason,
    }
}
