//! The coroutine protocol: validated request and answer types for the
//! yield/resume boundary between section Lua and the scheduler driver.
//!
//! A suspending host call (`models.infer`, `handle:infer`, `execute`,
//! `fanout`) is a Lua-side shim that yields a request table; the driver
//! validates the yield into a [`Request`], dispatches it, and resumes the
//! coroutine with the `(ok, result)` envelope rendered from an [`Answer`].
//!
//! The implementation lives in the `promptforge-lua` crate (the vocabulary is
//! produced by the Lua side) and is re-exported here unchanged, so existing
//! `crate::execute::protocol::*` paths keep working.

pub(crate) use promptforge_lua::{Answer, Request, YieldParse};
