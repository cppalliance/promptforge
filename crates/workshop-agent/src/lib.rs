//! Agent-program executor for the Workshop.
//!
//! Document prompts (`.md`) run via `promptforge_core::execute::run`; agent
//! programs (`.lua`) run via [`run_agent`] in this crate. The two are
//! sibling executors over the same substrate (`promptforge-lua`,
//! `promptforge-model-client`, `promptforge-tools`, `promptforge-store`,
//! `promptforge-core-support`); neither depends on the other.
//!
//! An agent program is one long-running Lua chunk driven as a single
//! coroutine. Its host surface is the shared kernel - `models.infer`,
//! `tool_call`, `store`, `log`, `var`, and cooperative cancellation - plus
//! the agent-only calls: `models.chat(messages, opts)`, one stateless
//! tool-capable model round that streams deltas to the host and returns
//! the reply or the unexecuted tool calls, and `runtime.events()`, a
//! read-only indexed view over the host-supplied event log whose snapshot
//! refreshes at every host-call resume. `execute()`, `fanout()`, and
//! `jump()` are absent - not stubbed - so an agent touching them fails as
//! an undefined global, exactly as a document prompt touching the
//! agent-only calls does.

mod agent;
mod config;

#[cfg(test)]
mod tests;

pub use agent::{AgentError, run_agent};
pub use config::{AgentConfig, AgentLimits};
