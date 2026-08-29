//! Cooperative cancellation for long-running execute paths.
//!
//! The implementation lives in the `promptforge-core-support` crate and is
//! re-exported here unchanged, so existing `promptforge_core::cancel::*` paths
//! keep working.

#[cfg(test)]
pub(crate) use promptforge_core_support::cancel::scope;
pub(crate) use promptforge_core_support::cancel::{
    CancelHandle, current, is_cancelled, maybe_scope, wait_cancelled,
};
