//! harness-capabilities - the harness's capability layer: the registry,
//! activation with co-activation conflict checking, and the [`Capability`]
//! and [`Tool`] traits the first-party capability crates implement.
//!
//! The engine holds none of this. It binds tool slots against descriptors
//! ([`promptforge::tools::ToolCatalog`]) and issues every tool
//! call as an effect naming an id; the implementations behind those ids
//! are defined here, in the harness. A host builds one
//! [`CapabilityRegistry`] of installed capabilities, calls [`activate`]
//! per run to turn a prompt's declarations into the run's catalog and its
//! [`ToolTable`] of implementations, hands the catalog to the engine's
//! `Environment`, and resolves each `ToolCall` effect in the table.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness/`; may depend on:
//!   `promptforge`, `gateway-api-types`, `gateway-api-discovery`,
//!   `shared-*`, and its container siblings.
//!   Never on a `workshop-*` crate, a private `gateway-*` crate, or a
//!   private `promptforge-*` crate. Read `AGENTS.md` before adding an
//!   import.
//! - This crate depends on no capability provider: the provider crates
//!   depend on it for the traits, never the reverse.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).

mod activation;
mod capability;
mod registry;
mod tool;

pub use activation::{Activation, ToolTable, activate};
pub use capability::{Capability, CapabilityError, CapabilityErrorKind, Contribution, RunServices};
pub use registry::{CapabilityRegistry, RegistryError, RegistryErrorKind};
pub use tool::Tool;

/// The capability identity vocabulary, re-exported from the engine's types
/// so a provider names one crate for the whole contract.
pub use promptforge::capabilities::{CapabilityId, CapabilityIdError, CapabilityIdErrorKind};
