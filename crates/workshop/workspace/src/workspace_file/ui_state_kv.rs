//! The opaque ui-state values a workspace file holds in its `kv` table
//! beside the window geometry: the dock layout, the expanded tree
//! folders, and the closed-editor stack. The server stores each
//! validated value's compact JSON text and never interprets it
//! further; the SPA owns each value's schema.

use std::collections::BTreeMap;

use serde_json::Value;
use workshop_support::StateBucketValue;

use super::actor::Command;
use super::{WorkspaceFile, WorkspaceFileError};
use crate::error::WorkspaceError;

/// The kv keys the workspace bucket accepts. Reserved and unused for
/// now: `scroll`, `agent_sessions`.
pub(crate) const UI_STATE_KEYS: [&str; 3] = ["layout", "tree", "closed_editors"];

/// The ui-state map with every key present and no value: what a file
/// with no ui-state rows reads as, and what save-as writes.
pub(crate) fn empty_ui_state() -> BTreeMap<&'static str, Option<Value>> {
    UI_STATE_KEYS.iter().map(|key| (*key, None)).collect()
}

impl WorkspaceFile {
    /// Stores the validated `put`'s JSON text under its key, replacing
    /// any earlier value.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::WorkspaceFileFailed`] when the write fails.
    pub(crate) async fn put_ui_state(&self, put: &StateBucketValue) -> Result<(), WorkspaceError> {
        let key = put.key();
        let json_text = put.text().to_owned();
        self.request(|reply| Command::PutUiState {
            key,
            json_text,
            reply,
        })
        .await?;
        Ok(())
    }

    /// Reads every ui-state value; a key with no row, or whose text no
    /// longer parses, reads as `None`. The workspace reads these values
    /// once, through [`WorkspaceFile::contents`] at open, and serves
    /// them from memory after that; this direct read is a test seam.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::WorkspaceFileFailed`] when the file
    /// cannot be read.
    #[cfg(test)]
    pub(crate) async fn read_ui_state(
        &self,
    ) -> Result<BTreeMap<&'static str, Option<Value>>, WorkspaceError> {
        Ok(self.contents().await?.ui_state)
    }
}

/// Inserts or replaces one ui-state row; runs on the actor.
pub(super) async fn put_ui_state_row(
    conn: &turso::Connection,
    key: &'static str,
    json_text: &str,
) -> Result<(), WorkspaceFileError> {
    conn.execute(
        "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
        (key, json_text),
    )
    .await?;
    Ok(())
}

/// Reads the ui-state rows into a map holding every allow-listed key;
/// runs on the actor. A row whose text no longer parses is treated as
/// absent with a warning: the SPA falls back to its default for that
/// one value rather than the file being refused.
pub(super) async fn read_ui_state_rows(
    conn: &turso::Connection,
) -> Result<BTreeMap<&'static str, Option<Value>>, WorkspaceFileError> {
    let mut state = empty_ui_state();
    for key in UI_STATE_KEYS {
        let mut rows = conn
            .query("SELECT value FROM kv WHERE key = ?1", (key,))
            .await?;
        let Some(row) = rows.next().await? else {
            continue;
        };
        let json: String = row.get(0)?;
        match serde_json::from_str(&json) {
            Ok(value) => {
                state.insert(key, Some(value));
            }
            Err(error) => {
                tracing::warn!(%error, key, "saved ui state does not parse; reading it as absent");
            }
        }
    }
    Ok(state)
}
