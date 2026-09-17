//! workshop-menu - the server-owned Model menu subsystem: the workbench
//! snapshot pushed to every `/ws` session as a `{"type":"workbench",...}`
//! frame, the chat-capable model catalog channel rebroadcast as
//! `{"type":"models",...}` frames, and the per-profile model memory
//! persisted in the state directory.
//!
//! ## Invariants
//!
//! - Tier: service; may depend on: `workshop-protocol`, `workshop-registry`,
//!   `workshop-support`. Read `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - The server owns all Model-menu state and the UI only renders it;
//!   `chat_ready` is computed here and never derived client-side.
//! - Publishing never blocks: a publish with no sessions is a no-op, and
//!   a lagging session skips ahead - every push is a complete snapshot, so
//!   an overwritten one loses nothing. Both buses retain the newest push,
//!   so a session connecting later receives the current state immediately.
//! - Mutation is zone two throughout: a refused mutation is a value
//!   returned to the caller, and a missing, unreadable, or corrupt memory
//!   file means "no memory yet" - logged and tolerated, never fatal.

pub mod catalog;
pub mod handles;
pub mod menu;

pub use catalog::{CatalogBus, ChatCatalog, is_chat_capable};
pub use handles::{MenuHandles, register};
pub use menu::{MenuBus, MenuRefusal, SwitchOutcome};
