//! Cooperative cancellation for the engine.
//!
//! The engine performs no I/O and awaits nothing, so it cannot select over
//! a cancellation token: it polls a flag between chain steps and from the
//! Lua instruction hook. That flag is the synchronous [`CancelHandle`] from
//! the `promptforge-api-types` crate, re-exported here so the crate's
//! `cancel::CancelHandle` path names the one handle a run carries.

pub(crate) use promptforge_api_types::cancel::CancelHandle;
