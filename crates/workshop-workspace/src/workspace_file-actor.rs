//! The workspace-file actor: one tokio task owning the one turso
//! connection, fed by a bounded channel. Channel order is disk order,
//! so the file mirrors memory without locks.

use std::path::{Path, PathBuf};

use tokio::sync::{mpsc, oneshot};

use super::{GrantRow, KV_WINDOW, META_NAME, WindowState, WorkspaceContents, WorkspaceFileError};

/// The size of a WAL file that holds a header and no frames. A sidecar
/// this small carries nothing the main file lacks.
const WAL_HEADER_SIZE: u64 = 32;

/// The v1 schema, exactly as applied at create time. `user_version` is
/// the migration counter; `meta` carries the format stamp and display
/// name; `grants` mirrors the in-memory grant set in tree order; `kv`
/// holds JSON values keyed by name (v1 defines only `window`).
pub(crate) const SCHEMA_V1: &str = "\
PRAGMA user_version = 1;

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE grants (
    path     TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    added_at TEXT NOT NULL
);

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// How many commands may wait in the channel before senders back off.
/// Grants arrive at user-gesture rate and geometry saves are debounced,
/// so a small bound is ample; a full channel is a symptom, not a mode.
pub(crate) const COMMAND_QUEUE_DEPTH: usize = 32;

/// One unit of work for the actor. Each variant that answers carries a
/// `oneshot` reply; a dropped reply means the caller gave up, which the
/// actor ignores.
#[derive(Debug)]
pub(crate) enum Command {
    /// Read everything the file holds.
    Contents {
        /// Where the contents go.
        reply: oneshot::Sender<Result<WorkspaceContents, WorkspaceFileError>>,
    },
    /// Stop the loop and close the connection; the reply fires once the
    /// connection is dropped, so a caller can reopen the file safely.
    Shutdown {
        /// Fires after the connection closes.
        reply: oneshot::Sender<()>,
    },
}

/// The actor loop: one task, one connection, channel order is disk
/// order. Ends when the last sender drops or a `Shutdown` arrives; either
/// way the connection closes through [`close_database`], so the file is
/// left alone at `path` with no sidecar beside it.
pub(crate) async fn run(mut rx: mpsc::Receiver<Command>, conn: turso::Connection, path: PathBuf) {
    let default_name = super::stem_of(&path);
    while let Some(command) = rx.recv().await {
        match command {
            Command::Contents { reply } => {
                let _ = reply.send(read_contents(&conn, &default_name).await);
            }
            Command::Shutdown { reply } => {
                // Close the channel so no later command outlives the
                // connection, then close the file before answering.
                rx.close();
                close_database(conn, &path).await;
                let _ = reply.send(());
                return;
            }
        }
    }
    close_database(conn, &path).await;
}

/// Closes the only connection to the database at `path` and leaves
/// exactly the file behind: the WAL is checkpointed into the main file
/// and truncated, the connection dropped, and the emptied `-wal` sidecar
/// removed. A sidecar that still holds frames is never touched; the
/// engine replays it on the next open.
pub(crate) async fn close_database(conn: turso::Connection, path: &Path) {
    if let Err(error) = conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", ()).await {
        tracing::warn!(%error, path = %path.display(), "workspace file checkpoint failed on close");
    }
    drop(conn);
    remove_empty_wal_sidecar(path);
}

/// Removes the `-wal` sidecar beside `path` when it holds no frames.
fn remove_empty_wal_sidecar(path: &Path) {
    let sidecar = wal_sidecar_of(path);
    let Ok(metadata) = std::fs::metadata(&sidecar) else {
        return;
    };
    if metadata.len() > WAL_HEADER_SIZE {
        tracing::warn!(
            path = %sidecar.display(),
            "workspace file wal sidecar still holds frames after close; leaving it in place"
        );
        return;
    }
    if let Err(error) = std::fs::remove_file(&sidecar) {
        tracing::warn!(%error, path = %sidecar.display(), "workspace file wal sidecar not removed");
    }
}

/// The path of the WAL sidecar the engine keeps beside `path`.
pub(crate) fn wal_sidecar_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// Reads the display name, the grants in position order, and the saved
/// window from an already validated connection.
pub(crate) async fn read_contents(
    conn: &turso::Connection,
    default_name: &str,
) -> Result<WorkspaceContents, WorkspaceFileError> {
    let name = read_meta(conn, META_NAME)
        .await?
        .unwrap_or_else(|| default_name.to_string());
    let grants = read_grants(conn).await?;
    let window_state = read_window(conn).await?;
    Ok(WorkspaceContents {
        name,
        grants,
        window_state,
    })
}

/// Reads one `meta` value; `None` when the key is absent.
pub(crate) async fn read_meta(
    conn: &turso::Connection,
    key: &str,
) -> Result<Option<String>, WorkspaceFileError> {
    let mut rows = conn
        .query("SELECT value FROM meta WHERE key = ?1", (key,))
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Reads the grants table in tree order.
async fn read_grants(conn: &turso::Connection) -> Result<Vec<GrantRow>, WorkspaceFileError> {
    let mut rows = conn
        .query(
            "SELECT path, position, added_at FROM grants ORDER BY position",
            (),
        )
        .await?;
    let mut grants = Vec::new();
    while let Some(row) = rows.next().await? {
        let path: String = row.get(0)?;
        grants.push(GrantRow {
            path: PathBuf::from(path),
            position: row.get(1)?,
            added_at: row.get(2)?,
        });
    }
    Ok(grants)
}

/// Reads the saved window geometry; `None` when never saved. A value
/// that no longer parses is treated as absent: the shell falls back to
/// its default geometry rather than refusing the whole file.
async fn read_window(conn: &turso::Connection) -> Result<Option<WindowState>, WorkspaceFileError> {
    let mut rows = conn
        .query("SELECT value FROM kv WHERE key = ?1", (KV_WINDOW,))
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let json: String = row.get(0)?;
    match serde_json::from_str(&json) {
        Ok(state) => Ok(Some(state)),
        Err(error) => {
            tracing::warn!(%error, "saved window state does not parse; using defaults");
            Ok(None)
        }
    }
}
