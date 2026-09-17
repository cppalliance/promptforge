//! Guard-wrapping for untrusted external data.
//!
//! The implementation lives in the `promptforge-api-types` crate and is
//! re-exported here unchanged, so existing `promptforge_api_runtime::untrusted::*`
//! paths keep working.

pub(crate) use promptforge_api_types::untrusted::GuardNonce;
