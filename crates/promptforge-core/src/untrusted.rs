//! Guard-wrapping for untrusted external data.
//!
//! The implementation lives in the `promptforge-core-support` crate and is
//! re-exported here unchanged, so existing `promptforge_core::untrusted::*`
//! paths keep working.

pub(crate) use promptforge_core_support::untrusted::{GuardNonce, wrap};
