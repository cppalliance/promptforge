//! harness-plugins - the Harness's Plugin layer: the registry,
//! activation with co-activation conflict checking, the [`Plugin`]
//! and [`Tool`] traits the first-party Plugin crates implement, and
//! one core Plugin of its own, [`UserInput`] (`promptforge/user-input`).
//!
//! The Engine holds none of this. It binds tool slots against descriptors
//! ([`promptforge::tools::ToolCatalog`]) and issues every tool
//! call as an effect naming an id; the implementations behind those ids
//! are defined here, in the Harness. The Host builds one
//! [`PluginRegistry`] of installed Plugins and a
//! [`HostServices`] map and hands both to the Harness, which calls
//! [`activate`] per run to turn a prompt's declarations into the run's
//! catalog, its preludes, and its [`ToolTable`] of implementations, hands
//! the catalog and the preludes to the Engine's `Environment`, and
//! resolves each `ToolCall` effect in the table.
//!
//! ## Invariants
//!
//! - This crate depends on no Plugin provider: the provider crates
//!   depend on it for the traits, never the reverse. The one Plugin
//!   it holds itself, `promptforge/user-input`, needs nothing beyond this
//!   crate's traits and the broker it receives through [`RunServices`].
//! - This crate names no async runtime: `tokio` and `tokio-util` appear
//!   only under `[dev-dependencies]` (enforced by the Harness tokio ban in
//!   `cargo test -p build-xtask`). The Harness polls every tool call and
//!   input wait inside the run's own future, so a [`Tool`] or an
//!   [`InputBroker`] must not block while polled.

mod activation;
mod input;
mod plugin;
mod registry;
mod service;
#[cfg(test)]
mod test_support;
mod tool;
mod user_input;

pub use activation::{Activation, ServiceGap, ToolTable, activate};
pub use input::{InputBroker, InputError};
pub use plugin::{Contribution, Plugin, PluginError, PluginErrorKind, RunServices};
pub use registry::{PluginRegistry, RegistryError, RegistryErrorKind};
pub use service::{HostServices, ServiceError, ServiceId, ServiceKey};
pub use tool::{Tool, ToolContext};
pub use user_input::{INPUT_BROKER, USER_INPUT_ASK_TOOL, UserInput};

/// The Plugin identity vocabulary, re-exported from the Engine's types
/// so a provider names one crate for the whole contract.
pub use promptforge::plugins::{PluginId, PluginIdError, PluginIdErrorKind};
