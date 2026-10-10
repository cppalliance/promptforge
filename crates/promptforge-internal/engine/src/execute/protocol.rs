//! The coroutine protocol: validated request and answer types for the
//! yield/resume boundary between section Lua and the scheduler driver.
//!
//! A suspending Engine call (`models.infer(prompt)`, `h:infer(prompt)`, `call`,
//! `fanout`, `tools.call`) is a Lua-side shim that yields a request table; the driver
//! validates the yield into a [`Request`], dispatches it, and resumes the
//! coroutine with the `(ok, result)` envelope rendered from an [`Answer`].
//!
//! The implementation is defined in the `promptforge-lua` crate (the
//! vocabulary is produced by the Lua side) and is re-exported here
//! unchanged, so existing `crate::execute::protocol::*` paths keep working.

pub(crate) use promptforge_lua::{
    Answer, ChatResult, LocalToolOutcome, MessageContent, MessageRecord, MessageRole, Request,
    TaskDelivery, TaskStatus, ToolCallOutcome, VfsOp, VfsOutcome, YieldParse,
};
