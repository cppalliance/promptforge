//! workshop-status - the status-bar subsystem: a broadcast bus for
//! status updates from every subsystem to every connected `/ws` session.
//! Work in flight reaches the bar as a busy frame pushed by whichever
//! subsystem owns the work.
//!
//! ## Invariants
//!
//! - Tier: service; may depend on: `workshop-protocol`, `workshop-registry`,
//!   `workshop-support`. Read `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Sending on the bus never blocks: a send with no subscribers is a
//!   no-op, and a lagging subscriber skips ahead, so instrumenting a hot
//!   path cannot stall the subsystem it observes.
//! - The bus retains the newest update, so a session that connects later
//!   sends the current status immediately - the delivery contract's
//!   resend-on-reconnect for ephemeral frames.
//! - The public API is infallible (sends are no-ops on lag or empty
//!   rings, never errors), so the crate has no thiserror error type -
//!   the same exemption `workshop-registry` takes.

pub mod status;

pub mod handles;

pub use handles::{StatusRegistrations, register};
pub use status::StatusBus;
