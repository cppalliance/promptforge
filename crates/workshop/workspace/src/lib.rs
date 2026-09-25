//! workshop-workspace - the workspace subsystem: confined filesystem
//! access behind `/workspace/*` - directory trees, file reads, and file
//! writes jailed to roots the user explicitly granted (drag and drop,
//! the folder picker, or a reopened workspace file) - plus the
//! `/workspace/file/*` routes that treat the workspace as a document:
//! one `.pfwork` Turso file holding the grants and the window geometry,
//! reopened at boot through the `state_dir/last-workspace` pointer.
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
//! - The in-memory grant set is the confinement source of truth; an
//!   optional workspace file (a single Turso database) mirrors it between
//!   sessions and is never consulted on a request path. A persist that
//!   fails is logged degradation; the in-memory state stands.
//! - The crate maps its own [`WorkspaceError`] to the wire envelope at
//!   its route boundary; no server error type appears here.

mod blocking;
mod error;
mod handlers;
pub mod handles;
mod workspace;
mod workspace_file;
#[cfg(feature = "test-fixtures")]
#[path = "workspace-stall.rs"]
mod workspace_stall;

pub use error::WorkspaceError;
pub use handlers::routes;
#[cfg(feature = "test-fixtures")]
pub use handlers::routes_with_deadline;
pub use handles::{WorkspaceRegistrations, register, register_tasks};
pub use workspace::{
    EntryKind, FileContents, GrantEntry, TreeEntry, TreeListing, Workspace, WorkspaceSummary,
};
#[cfg(any(test, feature = "test-fixtures"))]
pub use workspace_file::create_alien_database_for_test;
pub use workspace_file::{WindowState, WorkspaceFileError};
#[cfg(feature = "test-fixtures")]
pub use workspace_stall::WriteStallHandle;
