//! workshop-user-state - the account-scoped UI state bucket: the values
//! the Workshop SPA keeps per user rather than per workspace (editor
//! settings, zoom, recent files, command history), persisted as one
//! JSON file in the server's state directory and served over
//! `/user/state`.
//!
//! ## Invariants
//!
//! - Tier: feature; may depend on: `workshop-protocol`,
//!   `workshop-registry`, `workshop-support`. Never on
//!   `workshop-workspace`, `workshop-sessions`, `workshop-server`, or any
//!   `gateway-*` or `promptforge-*` crate. Read `AGENTS.md` before adding
//!   an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - The server stores each value verbatim and never interprets it
//!   beyond the allow-listed key and the size cap; the SPA owns every
//!   value's schema.
//! - One writer: every put updates the in-memory map under one mutex and
//!   rewrites the whole file through the shared atomic-write helper, so
//!   a crash leaves the old document or the new, never a truncation.
//! - Zone two throughout: a missing, unreadable, or corrupt file reads as
//!   "no state yet" - logged and tolerated; a refused put is a value
//!   returned to the caller and writes nothing.

mod error;
mod store;

use std::sync::Arc;

use workshop_registry::{Registration, Registry};

pub use error::UserStateError;
pub use store::{USER_STATE_KEYS, USER_STATE_VALUE_CAP, UserStateStore};

/// Registers the user-state subsystem into the registry: the store as
/// the subsystem's state handle, so the composition root fetches it by
/// slot instead of holding it by name. The returned guard keeps the
/// registration alive; the composition root holds it for the process
/// lifetime.
pub fn register(registry: &Registry, store: Arc<UserStateStore>) -> Registration {
    registry.register_state::<UserStateStore>(store)
}
