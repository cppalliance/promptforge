//! harness-capabilities - the harness's capability layer: the registry,
//! activation with co-activation conflict checking, the [`Capability`]
//! and [`Tool`] traits the first-party capability crates implement, and
//! one core capability of its own, [`UserInput`] (`promptforge/user-input`).
//!
//! The engine holds none of this. It binds tool slots against descriptors
//! ([`promptforge::tools::ToolCatalog`]) and issues every tool
//! call as an effect naming an id; the implementations behind those ids
//! are defined here, in the harness. A host builds one
//! [`CapabilityRegistry`] of installed capabilities, calls [`activate`]
//! per run to turn a prompt's declarations into the run's catalog, its
//! preludes, and its [`ToolTable`] of implementations, hands the catalog
//! and the preludes to the engine's `Environment`, and resolves each
//! `ToolCall` effect in the table.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness-internal/`; may depend
//!   on: `promptforge` and container siblings only.
//!   Never on a `workshop-*`, `gateway-*`, or `shared-*` crate, or a
//!   private `promptforge-*` crate. Read `AGENTS.md` before adding an
//!   import.
//! - This crate depends on no capability provider: the provider crates
//!   depend on it for the traits, never the reverse. The one capability
//!   it holds itself, `promptforge/user-input`, needs nothing beyond this
//!   crate's traits and the broker it receives through [`RunServices`].
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).

mod activation;
mod capability;
mod input;
mod registry;
mod tool;
mod user_input;

pub use activation::{Activation, ServiceGap, ToolTable, activate};
pub use capability::{
    Capability, CapabilityError, CapabilityErrorKind, Contribution, RunServices, Service,
};
pub use input::{InputBroker, InputError};
pub use registry::{CapabilityRegistry, RegistryError, RegistryErrorKind};
pub use tool::Tool;
pub use user_input::{USER_INPUT_ASK_TOOL, UserInput};

/// The capability identity vocabulary, re-exported from the engine's types
/// so a provider names one crate for the whole contract.
pub use promptforge::capabilities::{CapabilityId, CapabilityIdError, CapabilityIdErrorKind};
