//! workshop-server-api - the desktop shell's entire view of the workshop
//! server: re-exports only.
//!
//! The shell (`workshop`) depends on this crate and never on
//! `workshop-server`, so server internals do not resolve in the shell at
//! all. The surface is the configuration types, the in-process server
//! lifecycle, and the Gateway publication seam; the `test-fixtures`
//! feature forwards the server's integration-test seams.
//!
//! ## Invariants
//!
//! - Tier: shell boundary; may depend on: `workshop-server` only. Read
//!   `AGENTS.md` before adding an import.
//! - This crate is re-exports only: no types, functions, or logic of its
//!   own. Anything the shell needs is a `pub use` of a `workshop-server`
//!   item, and the shell's sole view of the server is this crate.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

pub use workshop_server::{
    AgentsConfig, Config, GatewayConfig, GatewayPublicationError, GatewayUpdater, ServerConfig,
    ServerHandle, SpawnError, Termination, spawn,
};

/// The server's integration-test seams, forwarded to the shell's tests.
#[cfg(feature = "test-fixtures")]
pub use workshop_server::fixtures;

#[cfg(test)]
#[path = "lib-tests.rs"]
mod tests;
