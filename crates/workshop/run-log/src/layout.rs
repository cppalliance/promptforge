//! The log's layout version: a file written in another layout is set
//! aside, never written into.
//!
//! Recorded runs are disposable, so a change to the tables carries no
//! migration. A file whose `layout` row names another version, or that
//! predates the `layout` table, is renamed beside its path together with
//! its write-ahead log, and a fresh file takes the path. Nothing is
//! deleted.

use std::path::{Path, PathBuf};

use crate::error::LogError;
use crate::schema;

/// The sidecar files Turso keeps beside a database, by suffix.
const SIDECARS: [&str; 2] = ["-wal", "-shm"];

/// Whether `conn`'s file holds tables in a layout other than
/// [`schema::LAYOUT_VERSION`]. A file with no `runs` table is new, not
/// stale. A `layout` table with no row is stale too: only an open cut
/// short between creating the tables and stamping them leaves one.
pub(crate) async fn is_stale(conn: &turso::Connection) -> Result<bool, LogError> {
    if !has_table(conn, "runs").await? {
        return Ok(false);
    }
    if !has_table(conn, "layout").await? {
        return Ok(true);
    }
    Ok(stamped(conn).await? != Some(schema::LAYOUT_VERSION))
}

/// Stamps `conn`'s file with [`schema::LAYOUT_VERSION`] unless it already
/// holds a stamp. Runs after the schema is applied.
pub(crate) async fn stamp(conn: &turso::Connection) -> Result<(), LogError> {
    if stamped(conn).await?.is_none() {
        conn.execute(schema::INSERT_LAYOUT, (schema::LAYOUT_VERSION,))
            .await?;
    }
    Ok(())
}

/// Renames the database at `path`, and each sidecar beside it, to
/// `<file name>.stale-<stamp>` in the same directory, and returns the
/// database's new path. Every connection to the file must already be
/// dropped: Windows refuses to rename an open file.
///
/// # Errors
/// Returns [`LogError::Io`] when a rename fails.
pub(crate) async fn set_aside(path: &Path, stamp: i64) -> Result<PathBuf, LogError> {
    let aside = with_suffix(path, &format!(".stale-{stamp}"));
    tokio::fs::rename(path, &aside).await?;
    for sidecar in SIDECARS {
        let from = with_suffix(path, sidecar);
        if tokio::fs::try_exists(&from).await? {
            tokio::fs::rename(&from, with_suffix(&aside, sidecar)).await?;
        }
    }
    Ok(aside)
}

/// The version the file was stamped with, `None` when it holds no stamp.
async fn stamped(conn: &turso::Connection) -> Result<Option<i64>, LogError> {
    let mut rows = conn.query(schema::SELECT_LAYOUT, ()).await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get::<i64>(0)?)),
        None => Ok(None),
    }
}

/// Whether `conn`'s file holds a table named `name`.
async fn has_table(conn: &turso::Connection, name: &str) -> Result<bool, LogError> {
    let mut rows = conn.query(schema::SELECT_TABLE, (name,)).await?;
    Ok(rows.next().await?.is_some())
}

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}
