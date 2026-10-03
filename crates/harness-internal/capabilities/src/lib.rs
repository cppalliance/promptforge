//! harness-capabilities - the Harness's capability layer: the registry,
//! activation with co-activation conflict checking, the [`Capability`]
//! and [`Tool`] traits the first-party capability crates implement, and
//! one core capability of its own, [`UserInput`] (`promptforge/user-input`).
//!
//! The Engine holds none of this. It binds tool slots against descriptors
//! ([`promptforge::tools::ToolCatalog`]) and issues every tool
//! call as an effect naming an id; the implementations behind those ids
//! are defined here, in the Harness. The Host builds one
//! [`CapabilityRegistry`] of installed capabilities and a
//! [`HostServices`] map and hands both to the Harness, which calls
//! [`activate`] per run to turn a prompt's declarations into the run's
//! catalog, its preludes, and its [`ToolTable`] of implementations, hands
//! the catalog and the preludes to the Engine's `Environment`, and
//! resolves each `ToolCall` effect in the table.
//!
//! ## Invariants
//!
//! - Family: Harness, private to `crates/harness-internal/`; may depend
//!   on: `promptforge` and container siblings only.
//!   Never on a `workshop-*`, `gateway-*`, or `shared-*` crate, or a
//!   private `promptforge-*` crate. `cargo test -p build-xtask` enforces the
//!   product and container boundaries.
//! - This crate depends on no capability provider: the provider crates
//!   depend on it for the traits, never the reverse. The one capability
//!   it holds itself, `promptforge/user-input`, needs nothing beyond this
//!   crate's traits and the broker it receives through [`RunServices`].
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Nothing in this crate spawns a tokio task (enforced by this crate's
//!   `clippy.toml`): the Harness polls every tool call and input wait
//!   inside the run's own future, so a [`Tool`] or an [`InputBroker`] must
//!   not block while polled.

mod activation;
mod capability;
mod input;
mod registry;
mod service;
mod tool;
mod user_input;

pub use activation::{Activation, ServiceGap, ToolTable, activate};
pub use capability::{Capability, CapabilityError, CapabilityErrorKind, Contribution, RunServices};
pub use input::{InputBroker, InputError};
pub use registry::{CapabilityRegistry, RegistryError, RegistryErrorKind};
pub use service::{HostServices, ServiceError, ServiceId, ServiceKey};
pub use tool::Tool;
pub use user_input::{INPUT_BROKER, USER_INPUT_ASK_TOOL, UserInput};

/// The capability identity vocabulary, re-exported from the Engine's types
/// so a provider names one crate for the whole contract.
pub use promptforge::capabilities::{CapabilityId, CapabilityIdError, CapabilityIdErrorKind};
