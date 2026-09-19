//! The coroutine protocol: validated request and answer types for the
//! yield/resume boundary between section Lua and the scheduler driver.
//!
//! A suspending host call (`models.infer(handle?, prompt)`, `call`,
//! `tasks.spawn`, `fanout`, `tools.call`, the section-only `user_input()`
//! and `store.*`,
//! the agent-only `models.chat`, and the `chat` and `tool_call` rounds the
//! section-only `models.loop` shim yields on the author's behalf) is a
//! Lua-side shim that yields a request table; the driver validates the
//! yield into a [`Request`], dispatches it, and resumes the coroutine with
//! the `(ok, result)` envelope rendered from an [`Answer`]. The two enums
//! are the audit surface: what a script can cause the host to do is one
//! short read, and each variant's fields are the compiler-checked
//! per-message contract.
//!
//! The submodules split the protocol along that boundary: `request` the
//! request vocabulary (the [`Request`] and [`StoreOp`] enums and the
//! message-record types), `answer` the answer vocabulary (the [`Answer`]
//! enum and its payload types), `parse` the yield-to-request validation,
//! and `render` the answer-to-envelope rendering.

mod answer;
mod parse;
mod render;
mod request;
#[cfg(test)]
mod tests;

pub use answer::{Answer, ChatResult, StoreOutcome, ToolCallOutcome, UserInputOutcome};
pub use parse::YieldParse;
pub use request::{
    ContentPart, MessageContent, MessageRecord, MessageRole, Request, StoreOp, ToolCallRecord,
};
