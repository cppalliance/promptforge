//! Cooperative cancellation for the engine.
//!
//! The engine is a pure state machine, so instead of selecting over a
//! cancellation token it polls a flag between chain steps and from the Lua
//! instruction hook. That flag is the synchronous [`CancelHandle`] from the
//! `promptforge-types` crate, re-exported here so the crate's
//! `cancel::CancelHandle` path names the one handle a run holds.

pub(crate) use promptforge_types::cancel::CancelHandle;
