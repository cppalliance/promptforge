//! Agent-program executor for the Workshop.
//!
//! Document prompts (`.md`) run via `promptforge_core::execute::run`; agent
//! programs (`.lua`) run via [`run_agent`] in this crate. The two are
//! sibling executors over the same substrate (`promptforge-lua`,
//! `promptforge-model-client`, `promptforge-tools`, `promptforge-store`,
//! `promptforge-core-support`); neither depends on the other.
//!
//! An agent program is one long-running Lua chunk driven as a single
//! coroutine. Its host surface is the shared kernel: `models.infer`,
//! `tool_call`, `store`, `log`, `var`, and cooperative cancellation.
//! `execute()`, `fanout()`, and `jump()` are absent - not stubbed - so an
//! agent touching them fails as an undefined global, exactly as a document
//! prompt touching the agent-only calls does.

mod agent;
mod config;

pub use agent::{AgentError, run_agent};
pub use config::{AgentConfig, AgentLimits};
