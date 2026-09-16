//! workshop-workspace - the workspace subsystem: confined filesystem
//! access behind `/workspace/*` - directory trees, file reads, and file
//! writes jailed to roots explicitly granted through drag and drop.
//!
//! ## Invariants
//!
//! - Tier: feature; may depend on: `workshop-protocol`,
//!   `workshop-registry`, `workshop-support`, and the service crates.
//!   Read `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - Every request path is checked lexically (no `..`, and on Windows no
//!   NTFS alternate data stream names) and then canonicalized and
//!   prefix-matched against the canonical grants before any filesystem
//!   operation, so traversal, symlink escapes, and UNC aliases cannot
//!   reach outside a grant.
//! - Grants live in memory for the running process only - profile
//!   persistence is a separate future consent decision.
//! - The crate maps its own [`WorkspaceError`] to the wire envelope at
//!   its route boundary; no shell error type appears here.

mod error;
mod handlers;
pub mod handles;
mod workspace;
mod workspace_file;

pub use error::WorkspaceError;
pub use handlers::routes;
pub use handles::register;
pub use workspace::{EntryKind, FileContents, TreeEntry, TreeListing, Workspace};
