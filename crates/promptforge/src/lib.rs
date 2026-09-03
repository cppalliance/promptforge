//! PromptForge integrator-facing facade.
//!
//! Two entry points: [`pipeline`] for document prompts (`.md`) and [`agent`]
//! for agent programs (`.lua`). This crate re-exports only; it never grows
//! logic or types of its own.
//!
//! Integrators who need substrate types (parser, store, tools, models)
//! depend on those crates directly.

/// Document prompts (`.md`): sections, prose, the built-in tool loop.
pub mod pipeline {
    pub use promptforge_core::execute::run;
    pub use promptforge_core::execute::{RunConfig, RunError};
}

/// Agent programs (`.lua`): the Lua program owns the loop.
pub mod agent {
    pub use promptforge_agent::{AgentConfig, AgentError, run_agent as run};
}
