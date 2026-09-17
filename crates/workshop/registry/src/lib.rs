//! workshop-registry - the keystone of the workshop server
//! decomposition: four contribution collections into which subsystems
//! self-register - routes and background tasks as ordered vectors of
//! trait objects, state handles and push sinks as maps keyed by type.
//! Consumers reach subsystems through the registry instead of by name,
//! so the composition root never hand-wires what a subsystem can
//! announce itself.
//!
//! ## Invariants
//!
//! - Tier: vocabulary; may depend on: `workshop-protocol` (the wire
//!   types the push-channel contributions carry). Read `AGENTS.md`
//!   before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Every subsystem trait is sealed (a private empty supertrait), so
//!   only this crate implements them: registrants plug in through the
//!   adapters provided here, never by implementing a trait downstream.
//! - Never add a field, slot, or accessor naming a subsystem; a new
//!   subsystem changes its own crate and one `register` call, never
//!   this crate.
//! - An unregistered contribution is a graceful no-op, never an error:
//!   consumers branch on `None` and continue degraded, and the [`Push`]
//!   facade drops intents whose sink is unregistered. The composition
//!   root alone is total: it calls [`Registry::require`] for the boot
//!   set, so a missing required contribution fails startup with a typed
//!   error naming the absent type.
//! - A registration is alive exactly as long as its guard: dropping the
//!   guard deregisters the contribution.

mod push;
mod registry;
mod traits;

pub use push::{MenuPush, Push};
pub use registry::{MissingContribution, Registration, Registry};
pub use traits::{
    BackgroundTask, BackgroundTaskAdapter, CatalogSink, CatalogSinkAdapter, MenuSink,
    MenuSinkAdapter, RouteRegistrar, RouteRegistrarAdapter, ShutdownHandle, StatusChannel,
    StatusChannelAdapter, StatusSink, StatusSinkAdapter, WorkspaceRoots, WorkspaceRootsAdapter,
};
