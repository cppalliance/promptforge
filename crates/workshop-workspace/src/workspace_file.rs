//! The workspace file: the single Turso database (`.pfwork`) that mirrors
//! the in-memory grant set and holds window state between sessions.
//!
//! This is the persistence seam of the workspace subsystem. The in-memory
//! grant set stays the confinement source of truth; nothing here is
//! consulted on a request path. All database I/O is async because turso's
//! API is async-native.
//!
//! This step is the dependency spike: it proves the crate opens a database
//! at a chosen path and round-trips a pragma. The schema, the
//! single-writer actor, and the typed contents land in the steps that
//! follow.

use std::io;
use std::path::Path;

/// A workspace-file operation failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkspaceFileError {
    /// The path could not be handed to the database engine.
    #[error("workspace file path cannot be used")]
    Io {
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// The database engine refused the operation.
    #[error("workspace database operation failed")]
    Database {
        /// The underlying engine failure.
        #[source]
        source: turso::Error,
    },
}

impl From<turso::Error> for WorkspaceFileError {
    fn from(source: turso::Error) -> Self {
        Self::Database { source }
    }
}

/// Opens the database at `path`, creating the file when absent, and
/// returns one connection to it.
///
/// The connection keeps the database alive; the caller owns nothing else.
/// The parent directory must already exist: this creates exactly the
/// file, never a directory.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "consumed by the schema step that follows the spike"
    )
)]
pub(crate) async fn open_database(path: &Path) -> Result<turso::Connection, WorkspaceFileError> {
    // turso addresses databases by string, so a path that is not UTF-8
    // cannot be opened at all; surface that as the I/O failure it is.
    let path = path.to_str().ok_or_else(|| WorkspaceFileError::Io {
        source: io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace file path must be utf-8",
        ),
    })?;
    let database = turso::Builder::new_local(path).build().await?;
    Ok(database.connect()?)
}

#[cfg(test)]
#[path = "workspace-file-tests.rs"]
mod tests;
