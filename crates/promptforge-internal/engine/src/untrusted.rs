//! Guard-wrapping for untrusted external data.
//!
//! The implementation sits in the `promptforge-types` crate's
//! `untrusted` module; this module is the crate-internal import surface for
//! it.

pub(crate) use promptforge_types::untrusted::GuardNonce;
