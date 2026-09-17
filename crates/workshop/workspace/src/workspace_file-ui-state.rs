//! The opaque ui-state values a workspace file holds in its `kv` table
//! beside the window geometry: the dock layout, the expanded tree
//! folders, and the closed-editor stack. The server stores the text
//! the client sent, verbatim, and never interprets it beyond checking
//! that it parses as JSON and fits under the cap; the SPA owns each
//! value's schema.

use std::collections::BTreeMap;

use serde_json::Value;

use super::actor::Command;
use super::{WorkspaceFile, WorkspaceFileError};
use crate::error::WorkspaceError;

/// The kv keys the workspace bucket accepts. Reserved and unused for
/// now: `scroll`, `agent_sessions`.
pub(crate) const UI_STATE_KEYS: [&str; 3] = ["layout", "tree", "closed_editors"];

/// The largest ui-state value the file accepts, in bytes of JSON text.
pub(crate) const UI_STATE_VALUE_CAP: usize = 1 << 20;

/// The ui-state map with every key present and no value: what a file
/// with no ui-state rows reads as, and what save-as writes.
pub(crate) fn empty_ui_state() -> BTreeMap<&'static str, Option<Value>> {
    UI_STATE_KEYS.iter().map(|key| (*key, None)).collect()
}

/// Resolves `key` to its allow-list entry.
///
/// # Errors
/// Returns [`WorkspaceError::UiStateKey`] when `key` is not one of
/// [`UI_STATE_KEYS`].
pub(crate) fn ui_state_key(key: &str) -> Result<&'static str, WorkspaceError> {
    UI_STATE_KEYS
        .iter()
        .copied()
        .find(|allowed| *allowed == key)
        .ok_or_else(|| WorkspaceError::UiStateKey(key.to_string()))
}

/// Checks that a value whose JSON text is `len` bytes fits under the cap.
///
/// # Errors
/// Returns [`WorkspaceError::UiStateTooLarge`] past the cap.
pub(crate) fn check_ui_state_cap(len: usize) -> Result<(), WorkspaceError> {
    if len > UI_STATE_VALUE_CAP {
        return Err(WorkspaceError::UiStateTooLarge {
            actual: len,
            cap: UI_STATE_VALUE_CAP,
        });
    }
    Ok(())
}

/// Checks that `json_text` fits under the cap and parses as JSON. The
/// cap is checked first so oversized text is never parsed.
///
/// # Errors
/// Returns [`WorkspaceError::UiStateTooLarge`] past the cap and
/// [`WorkspaceError::UiStateNotJson`] for text that does not parse.
pub(crate) fn check_ui_state_text(json_text: &str) -> Result<(), WorkspaceError> {
    check_ui_state_cap(json_text.len())?;
    serde_json::from_str::<serde::de::IgnoredAny>(json_text)
        .map(|_| ())
        .map_err(|_| WorkspaceError::UiStateNotJson)
}

impl WorkspaceFile {
    /// Stores `json_text` verbatim under the allow-listed `key`,
    /// replacing any earlier value. Validation runs before anything is
    /// sent to the actor, so a refused put writes nothing.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::UiStateKey`], [`WorkspaceError::UiStateTooLarge`],
    /// or [`WorkspaceError::UiStateNotJson`] for a refused input, and
    /// [`WorkspaceError::WorkspaceFileFailed`] when the write fails.
    pub(crate) async fn put_ui_state(
        &self,
        key: &str,
        json_text: &str,
    ) -> Result<(), WorkspaceError> {
        let key = ui_state_key(key)?;
        check_ui_state_text(json_text)?;
        let json_text = json_text.to_owned();
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

/// Reads the ui-state rows into a map carrying every allow-listed key;
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
