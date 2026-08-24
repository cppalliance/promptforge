//! PromptForge Workbench HTTP server.
//!
//! Holds the axum router and the serving loop so `src/main.rs` stays a thin
//! shell. Start at [`router`]; [`run`] binds and serves with the defaults.

mod app;

pub use app::{DEFAULT_ADDR, router, run};
