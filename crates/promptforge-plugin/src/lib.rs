//! `promptforge-plugin` - the contract a Plugin crate implements, and the
//! one crate it depends on.
//!
//! A Plugin crate exports a [`Package`]: its `vendor/name`, an optional
//! Lua prelude, the per-run services its calls read, and the `construct`
//! function the Host calls once, at install, to build the one [`Plugin`]
//! object every run shares. [`Plugin::call`] performs one tool call with
//! the [`ToolContext`] the Harness lends it. [`HostServices`] maps service
//! ids to the shared objects a Host provides, each read back through a
//! typed [`ServiceKey`].
//!
//! The Engine never depends on this crate; it sees only tool descriptors,
//! preludes, and ids. The Engine and filesystem names a Plugin author
//! needs are re-exported here, so a Plugin crate names no other
//! promptforge crate.
//!
//! The `test-support` feature adds `testing::TestCall`, which lends a
//! [`ToolContext`] to a Plugin crate's own tests.
//!
//! ## Invariants
//!
//! - May depend on: `promptforge-types`, `promptforge-vfs`, and
//!   `workspace-hack` among workspace crates. As an Engine crate it
//!   declares no async runtime, no `async-trait`, and no HTTP client.
//!   `cargo test -p build-xtask` enforces the Engine manifest guard and
//!   the product and container boundaries.
//! - Only `[dev-dependencies]` enable `test-support`, which the
//!   `test-support` leak guard enforces.
//! - A Host builds each Plugin object once, through
//!   [`Package::construct`], and every run shares it. Host-wide services
//!   reach only `construct`, and a run's own services reach only calls,
//!   through [`ToolContext::service`]. Cleanup goes in the Plugin's
//!   `Drop`; there is no shutdown hook.
//! - Every tool a Plugin offers sits under the name `construct` receives,
//!   as `web/fetch` sits under `web`. A tool that does not is dropped from
//!   every run's catalog.
//! - [`Plugin::call`] must not block while polled. Every effect of a run
//!   is polled on the same task, so blocking or CPU-heavy work goes to the
//!   Host's runtime.
//! - [`Plugin::call`] must not panic. A panicking call's effect is
//!   answered `Dropped`, and the panic is logged.
//! - [`Plugin::call`] marks the trust of every output correctly: output
//!   that embeds data an attacker can influence is
//!   [`ToolOutput::untrusted`].
//! - [`Plugin::call`] is cancellation-aware. Its future is dropped on a
//!   stop or a cancel, except that a stop spares a call whose tool's
//!   descriptor sets `survives_stop`.

mod context;
mod plugin;
mod service;
#[cfg(feature = "test-support")]
pub mod testing;

pub use context::ToolContext;
pub use plugin::{Package, Plugin, PluginFuture};
pub use service::{HostServices, ServiceError, ServiceId, ServiceKey};

pub use promptforge_types::plugins::{PluginId, PluginIdError, PluginIdErrorKind};
pub use promptforge_types::tools::{
    OutputTrust, ToolCallOrigin, ToolCaller, ToolDescriptor, ToolError, ToolErrorKind, ToolId,
    ToolIdError, ToolIdErrorKind, ToolOutput,
};
pub use promptforge_vfs::{Access, Entry, FileType, PathReason, Stat, VfsError};
