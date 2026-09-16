//! The workspace-file actor: one tokio task owning the one turso
//! connection, fed by a bounded channel. Channel order is disk order,
//! so the file mirrors memory without locks.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tokio::sync::{mpsc, oneshot};

use super::{
    FORMAT_NAME, GrantRow, KV_WINDOW, META_CREATED_AT, META_FORMAT, META_NAME, META_VERSION,
    SUPPORTED_VERSION, WindowState, WorkspaceContents, WorkspaceFileError,
};

/// A reply slot for a command that answers with success or failure.
type Ack = oneshot::Sender<Result<(), WorkspaceFileError>>;

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
    /// Insert or replace one grant; the file assigns its `position`.
    AddGrant {
        /// The grant to persist.
        row: GrantRow,
        /// Fires once the row is written.
        reply: Ack,
    },
    /// Delete the grant at `path`; the survivors keep their positions.
    RemoveGrant {
        /// The canonical root to forget.
        path: PathBuf,
        /// Fires once the row is gone.
        reply: Ack,
    },
    /// Insert or replace the saved window geometry.
    PutWindowState {
        /// The geometry to persist.
        state: WindowState,
        /// Fires once the value is written.
        reply: Ack,
    },
    /// Fold the WAL into the main file, then copy the main file to
    /// `destination`, all on the actor so no write lands in between and
    /// no checkpoint runs during the copy.
    Snapshot {
        /// Where the complete copy goes; must not exist.
        destination: PathBuf,
        /// Fires once the copy is on disk.
        reply: Ack,
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
            Command::AddGrant { row, reply } => {
                let _ = reply.send(add_grant(&conn, &row).await);
            }
            Command::RemoveGrant { path, reply } => {
                let _ = reply.send(remove_grant(&conn, &path).await);
            }
            Command::PutWindowState { state, reply } => {
                let _ = reply.send(put_window_state(&conn, state).await);
            }
            Command::Snapshot { destination, reply } => {
                let _ = reply.send(snapshot(&conn, &path, &destination).await);
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
    if let Err(error) = checkpoint(&conn).await {
        tracing::warn!(%error, path = %path.display(), "workspace file checkpoint failed on close");
    }
    drop(conn);
    remove_empty_wal_sidecar(path);
}

/// Folds every WAL frame into the main file and truncates the WAL, so
/// the main file alone is a complete copy of the database.
async fn checkpoint(conn: &turso::Connection) -> Result<(), WorkspaceFileError> {
    // The pragma answers with a status row (busy, log, checkpointed);
    // it must be read as a query and drained for the work to complete.
    let mut rows = conn.query("PRAGMA wal_checkpoint(TRUNCATE)", ()).await?;
    while rows.next().await?.is_some() {}
    Ok(())
}

/// Checkpoints the database at `path` and copies the main file to
/// `destination`. Running on the actor, between commands, this is the
/// one moment the main file is guaranteed complete and still: the
/// actor owns the only connection, so nothing writes or checkpoints
/// until the copy returns. The copy is a small synchronous file copy on
/// the actor task, the same blocking discipline as [`close_database`].
async fn snapshot(
    conn: &turso::Connection,
    path: &Path,
    destination: &Path,
) -> Result<(), WorkspaceFileError> {
    checkpoint(conn).await?;
    std::fs::copy(path, destination)
        .map(|_| ())
        .map_err(|source| WorkspaceFileError::Io { source })
}

/// Writes the stamp, the grants, and the window state into a freshly
/// schema'd database, all in one transaction.
pub(crate) async fn write_contents(
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

/// The row inserts behind [`write_contents`]. Grants land with the
/// positions they carry: create trusts its caller's order.
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
        insert_grant(conn, INSERT_GRANT, grant, grant.position).await?;
    }
    if let Some(window) = &contents.window_state {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)",
            (KV_WINDOW, window_json(window)),
        )
        .await?;
    }
    Ok(())
}

/// Inserts or replaces `row` at one past the file's current maximum
/// position, so the grants table keeps insertion order across sessions.
/// The row's own `position` is not consulted: only the file knows its
/// maximum. Re-granting a path it already holds moves it to the end.
async fn add_grant(conn: &turso::Connection, row: &GrantRow) -> Result<(), WorkspaceFileError> {
    let position = next_position(conn).await?;
    insert_grant(conn, REPLACE_GRANT, row, position).await
}

/// The create-time insert: a path given twice is a caller bug and fails
/// on the primary key rather than silently collapsing.
const INSERT_GRANT: &str = "INSERT INTO grants (path, position, added_at) VALUES (?1, ?2, ?3)";
/// The `add_grant` insert: a held path moves to the new position.
const REPLACE_GRANT: &str =
    "INSERT OR REPLACE INTO grants (path, position, added_at) VALUES (?1, ?2, ?3)";

/// One past the maximum `position` in the grants table; `0` when empty.
async fn next_position(conn: &turso::Connection) -> Result<u32, WorkspaceFileError> {
    let mut rows = conn
        .query("SELECT COALESCE(MAX(position), -1) + 1 FROM grants", ())
        .await?;
    match rows.next().await? {
        // The sum is never negative; a table past `u32::MAX` grants can
        // only saturate.
        Some(row) => Ok(u32::try_from(row.get::<i64>(0)?).unwrap_or(u32::MAX)),
        None => Ok(0),
    }
}

/// Writes `row` at `position` with `statement`, one of [`INSERT_GRANT`]
/// and [`REPLACE_GRANT`].
async fn insert_grant(
    conn: &turso::Connection,
    statement: &str,
    row: &GrantRow,
    position: u32,
) -> Result<(), WorkspaceFileError> {
    conn.execute(
        statement,
        (
            row.path.to_string_lossy().into_owned(),
            position,
            row.added_at.as_str(),
        ),
    )
    .await?;
    Ok(())
}

/// Deletes the grant at `path`. Positions are never renumbered: the
/// survivors keep theirs and the next grant takes one past the maximum.
async fn remove_grant(conn: &turso::Connection, path: &Path) -> Result<(), WorkspaceFileError> {
    conn.execute(
        "DELETE FROM grants WHERE path = ?1",
        (path.to_string_lossy().into_owned(),),
    )
    .await?;
    Ok(())
}

/// Inserts or replaces the saved window geometry.
async fn put_window_state(
    conn: &turso::Connection,
    state: WindowState,
) -> Result<(), WorkspaceFileError> {
    conn.execute(
        "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
        (KV_WINDOW, window_json(&state)),
    )
    .await?;
    Ok(())
}

/// The kv text for a window state. A five-field struct of plain numbers
/// and a bool always serializes; the Result is a trait artifact.
fn window_json(state: &WindowState) -> String {
    serde_json::to_string(state).unwrap_or_default()
}

/// The current time as RFC 3339 in UTC, whole seconds.
fn now_rfc3339() -> String {
    humantime::format_rfc3339_seconds(SystemTime::now()).to_string()
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
