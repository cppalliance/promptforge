//! The workspace file: the single Turso database (`.pfwork`) that mirrors
//! the in-memory grant set and holds window state between sessions.
//!
//! This is the persistence seam of the workspace subsystem. The in-memory
//! grant set stays the confinement source of truth; nothing here is
//! consulted on a request path. All database I/O is async because turso's
//! API is async-native, and it all runs on one actor task that owns the
//! one connection ([`actor::run`]): channel order is disk order.
//!
//! Schema v1 holds three tables: `meta`, `grants`, and `kv`. These table
//! names are reserved for follow-on projects and unused in v1:
//! `agent_windows`, `run_presets`, `runs`, `run_events`, `agents`,
//! `documents`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{fs, io};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

#[path = "workspace_file-actor.rs"]
mod actor;

use actor::{COMMAND_QUEUE_DEPTH, Command, SCHEMA_V1};

/// Meta key naming the file format; always [`FORMAT_NAME`].
pub(crate) const META_FORMAT: &str = "format";
/// Meta key carrying the schema version; always [`SUPPORTED_VERSION`]
/// for files this build writes.
pub(crate) const META_VERSION: &str = "version";
/// Meta key carrying the display name; absent means "use the file stem".
pub(crate) const META_NAME: &str = "name";
/// Meta key carrying the RFC 3339 creation time.
pub(crate) const META_CREATED_AT: &str = "created_at";
/// The one kv key v1 defines: the shell's saved geometry as JSON.
pub(crate) const KV_WINDOW: &str = "window";

/// The value every workspace file carries under [`META_FORMAT`].
pub(crate) const FORMAT_NAME: &str = "promptforge-workspace";
/// The schema version this build reads and writes, as both the `meta`
/// text and the `user_version` pragma.
pub(crate) const SUPPORTED_VERSION: &str = "1";
/// [`SUPPORTED_VERSION`] as the pragma integer.
const SUPPORTED_USER_VERSION: i64 = 1;

/// A workspace-file operation failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkspaceFileError {
    /// The path could not be handed to the database engine, or does
    /// not name a usable file.
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

    /// The file is not a PromptForge workspace: not a database, or a
    /// database without the `meta` stamp.
    #[error("{} is not a promptforge workspace file", path.display())]
    NotAWorkspace {
        /// The refused path.
        path: PathBuf,
    },

    /// The file is a workspace written by a build with a different
    /// schema version.
    #[error(
        "workspace file version {found} is unsupported; this build supports version {supported}"
    )]
    UnsupportedVersion {
        /// The version the file declares.
        found: String,
        /// The version this build supports.
        supported: &'static str,
    },

    /// The actor behind the handle has stopped.
    #[error("workspace file is closed")]
    Closed,
}

impl From<turso::Error> for WorkspaceFileError {
    fn from(source: turso::Error) -> Self {
        Self::Database { source }
    }
}

/// Everything a workspace file carries between sessions.
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceContents {
    /// Display name (meta 'name'); defaults to the file stem.
    pub(crate) name: String,
    /// Granted roots in tree order (grants table, ordered by position).
    pub(crate) grants: Vec<GrantRow>,
    /// Saved window geometry (kv 'window'), absent when never saved.
    pub(crate) window_state: Option<WindowState>,
}

/// One row of the grants table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantRow {
    /// Canonical granted root, verbatim-prefix-free.
    pub(crate) path: PathBuf,
    /// Stable tree order.
    pub(crate) position: u32,
    /// RFC 3339 grant time.
    pub(crate) added_at: String,
}

/// The kv 'window' value: the shell's saved geometry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct WindowState {
    /// Logical width.
    pub(crate) width: u32,
    /// Logical height.
    pub(crate) height: u32,
    /// Logical x position.
    pub(crate) x: i32,
    /// Logical y position.
    pub(crate) y: i32,
    /// Whether the window is maximized.
    pub(crate) maximized: bool,
}

/// A handle to an open workspace file: a clone-cheap sender into the
/// actor that owns the connection. Dropping every handle ends the actor
/// and closes the file; [`WorkspaceFile::close`] does the same and waits
/// for it.
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceFile {
    tx: mpsc::Sender<Command>,
}

impl WorkspaceFile {
    /// Creates a new workspace file at `path` holding `contents`, and
    /// returns a handle to it.
    ///
    /// Writes exactly one file at the chosen path: the parent directory
    /// must exist and the path must not. The schema is applied first;
    /// the `meta` stamp, the grants, and the window state then land in
    /// one transaction. A failure after the file appears removes it
    /// again, so a retry at the same path is not refused.
    pub(crate) async fn create(
        path: &Path,
        contents: &WorkspaceContents,
    ) -> Result<Self, WorkspaceFileError> {
        if path.exists() {
            return Err(WorkspaceFileError::Io {
                source: io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "workspace file path is already taken",
                ),
            });
        }
        let conn = open_database(path).await?;
        if let Err(error) = initialize(&conn, contents).await {
            // The connection must go before the file can; a failed
            // removal cannot say more than the write failure did.
            drop(conn);
            let _ = fs::remove_file(actor::wal_sidecar_of(path));
            let _ = fs::remove_file(path);
            return Err(error);
        }
        Ok(Self::spawn(conn, path))
    }

    /// Opens the workspace file at `path` after validating its stamp.
    ///
    /// Validation reads `user_version`, `meta.format`, and
    /// `meta.version` before anything else and writes nothing; a file
    /// that fails it is left byte-identical. The path must already
    /// exist: opening never creates.
    pub(crate) async fn open(path: &Path) -> Result<Self, WorkspaceFileError> {
        if !path.is_file() {
            return Err(WorkspaceFileError::Io {
                source: io::Error::new(io::ErrorKind::NotFound, "workspace file does not exist"),
            });
        }
        let conn = match open_database(path).await {
            Ok(conn) => conn,
            Err(error) if is_not_a_database(&error) => {
                return Err(WorkspaceFileError::NotAWorkspace {
                    path: path.to_path_buf(),
                });
            }
            Err(error) => return Err(error),
        };
        validate(&conn, path).await?;
        Ok(Self::spawn(conn, path))
    }

    /// Reads everything the file holds.
    pub(crate) async fn contents(&self) -> Result<WorkspaceContents, WorkspaceFileError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(Command::Contents { reply })
            .await
            .map_err(|_| WorkspaceFileError::Closed)?;
        response.await.map_err(|_| WorkspaceFileError::Closed)?
    }

    /// Stops the actor and waits for the connection to close, so the
    /// file can be reopened, copied, or removed immediately afterwards.
    /// A handle whose actor already stopped returns at once.
    pub(crate) async fn close(&self) {
        let (reply, response) = oneshot::channel();
        if self.tx.send(Command::Shutdown { reply }).await.is_ok() {
            let _ = response.await;
        }
    }

    /// Starts the actor over `conn`, the connection to the file at
    /// `path`, and returns its handle.
    fn spawn(conn: turso::Connection, path: &Path) -> Self {
        let (tx, rx) = mpsc::channel(COMMAND_QUEUE_DEPTH);
        tokio::spawn(actor::run(rx, conn, path.to_path_buf()));
        Self { tx }
    }
}

/// Applies the v1 schema to a freshly created database, then writes
/// `contents` into it in one transaction.
async fn initialize(
    conn: &turso::Connection,
    contents: &WorkspaceContents,
) -> Result<(), WorkspaceFileError> {
    conn.execute_batch(SCHEMA_V1).await?;
    write_contents(conn, contents).await
}

/// Opens the database at `path`, creating the file when absent, and
/// returns one connection to it.
///
/// The connection keeps the database alive; the caller owns nothing else.
/// The parent directory must already exist: this creates exactly the
/// file, never a directory.
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

/// Checks the stamp of an opened database without writing anything:
/// the `meta` table must exist and carry the format name, and both the
/// `user_version` pragma and `meta.version` must be the supported version.
async fn validate(conn: &turso::Connection, path: &Path) -> Result<(), WorkspaceFileError> {
    let refused = || WorkspaceFileError::NotAWorkspace {
        path: path.to_path_buf(),
    };
    let user_version = match read_user_version(conn).await {
        Ok(version) => version,
        // A file with a foreign header fails here rather than at open.
        Err(error) if is_not_a_database(&error) => return Err(refused()),
        Err(error) => return Err(error),
    };
    if !has_meta_table(conn).await? {
        return Err(refused());
    }
    match actor::read_meta(conn, META_FORMAT).await? {
        Some(format) if format == FORMAT_NAME => {}
        _ => return Err(refused()),
    }
    let version = actor::read_meta(conn, META_VERSION)
        .await?
        .ok_or_else(refused)?;
    if version != SUPPORTED_VERSION {
        return Err(WorkspaceFileError::UnsupportedVersion {
            found: version,
            supported: SUPPORTED_VERSION,
        });
    }
    if user_version != SUPPORTED_USER_VERSION {
        return Err(WorkspaceFileError::UnsupportedVersion {
            found: user_version.to_string(),
            supported: SUPPORTED_VERSION,
        });
    }
    Ok(())
}

/// Reads the `user_version` pragma.
async fn read_user_version(conn: &turso::Connection) -> Result<i64, WorkspaceFileError> {
    let mut rows = conn.query("PRAGMA user_version", ()).await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(0),
    }
}

/// Whether the schema declares a `meta` table.
async fn has_meta_table(conn: &turso::Connection) -> Result<bool, WorkspaceFileError> {
    let mut rows = conn
        .query(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = 'meta'",
            (),
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(row.get::<i64>(0)? > 0),
        None => Ok(false),
    }
}

/// Writes the stamp, the grants, and the window state into a freshly
/// schema'd database, all in one transaction.
async fn write_contents(
    conn: &turso::Connection,
    contents: &WorkspaceContents,
) -> Result<(), WorkspaceFileError> {
    conn.execute("BEGIN", ()).await?;
    let result = write_contents_rows(conn, contents).await;
    if result.is_err() {
        // A failed rollback cannot say more than the write failure did.
        let _ = conn.execute("ROLLBACK", ()).await;
        return result;
    }
    conn.execute("COMMIT", ()).await?;
    Ok(())
}

/// The row inserts behind [`write_contents`].
async fn write_contents_rows(
    conn: &turso::Connection,
    contents: &WorkspaceContents,
) -> Result<(), WorkspaceFileError> {
    const INSERT_META: &str = "INSERT INTO meta (key, value) VALUES (?1, ?2)";
    conn.execute(INSERT_META, (META_FORMAT, FORMAT_NAME))
        .await?;
    conn.execute(INSERT_META, (META_VERSION, SUPPORTED_VERSION))
        .await?;
    conn.execute(INSERT_META, (META_NAME, contents.name.as_str()))
        .await?;
    conn.execute(INSERT_META, (META_CREATED_AT, now_rfc3339()))
        .await?;
    for grant in &contents.grants {
        conn.execute(
            "INSERT INTO grants (path, position, added_at) VALUES (?1, ?2, ?3)",
            (
                grant.path.to_string_lossy().into_owned(),
                grant.position,
                grant.added_at.as_str(),
            ),
        )
        .await?;
    }
    if let Some(window) = &contents.window_state {
        // A five-field struct of plain numbers and a bool always
        // serializes; the Result is a trait artifact.
        let json = serde_json::to_string(window).unwrap_or_default();
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)",
            (KV_WINDOW, json),
        )
        .await?;
    }
    Ok(())
}

/// Whether an engine failure says the file is not a database at all.
fn is_not_a_database(error: &WorkspaceFileError) -> bool {
    matches!(
        error,
        WorkspaceFileError::Database {
            source: turso::Error::NotAdb(_) | turso::Error::Corrupt(_)
        }
    )
}

/// The file stem of `path`, the display name a file falls back to.
fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The current time as RFC 3339 in UTC, whole seconds.
fn now_rfc3339() -> String {
    humantime::format_rfc3339_seconds(SystemTime::now()).to_string()
}

#[cfg(test)]
#[path = "workspace-file-tests.rs"]
mod tests;
