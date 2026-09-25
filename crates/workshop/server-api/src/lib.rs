//! workshop-server-api - the desktop app's entire view of the workshop
//! server: re-exports only.
//!
//! The desktop app (`workshop`) depends on this crate and never on
//! `workshop-server`, so server internals do not resolve in the desktop app at
//! all. The surface is the configuration types, the in-process server
//! lifecycle, and the Gateway publication seam; the `test-fixtures`
//! feature forwards the server's integration-test seams.
//!
//! ## Invariants
//!
//! - Tier: desktop-app boundary; may depend on: `workshop-server` only. Read
//!   the repository-root `AGENTS.md` before adding an import.
//! - This crate is re-exports only: no types, functions, or logic of its
//!   own. Anything the desktop app needs is a `pub use` of a `workshop-server`
//!   item, and the desktop app's sole view of the server is this crate.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

pub use workshop_server::{
    AgentsConfig, Config, GatewayConfig, GatewayPublicationError, GatewayUpdater, ServerConfig,
    ServerHandle, SpawnError, Termination, spawn,
};

/// The server's integration-test seams, forwarded to the desktop app's tests.
#[cfg(feature = "test-fixtures")]
pub use workshop_server::fixtures;

#[cfg(test)]
#[path = "lib-tests.rs"]
mod tests;
