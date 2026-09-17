//! Cooperative cancellation for long-running execute paths.
//!
//! The implementation lives in the `promptforge-api-types` crate and is
//! re-exported here unchanged, so existing `promptforge_api_runtime::cancel::*` paths
//! keep working.

pub(crate) use promptforge_api_types::cancel::{
    CancelHandle, current, is_cancelled, maybe_scope, wait_cancelled,
};
