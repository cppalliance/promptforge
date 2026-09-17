//! The workspace-level view of the opaque ui-state values: an in-memory
//! map on the backing, read from the file at open and updated on every
//! accepted put, mirrored into the file through the actor. Memory is
//! the source of truth, as it is for grants and window state: a persist
//! that fails is logged and the value stands (zone two).

use std::collections::BTreeMap;
use std::sync::PoisonError;

use serde_json::Value;

use crate::Workspace;
use crate::error::WorkspaceError;
use crate::workspace_file::{check_ui_state_cap, empty_ui_state, ui_state_key};

impl Workspace {
    /// The ui-state values as memory holds them: every allow-listed key,
    /// each `None` until put or read from the file at open. An ephemeral
    /// workspace has nowhere to keep them and reports every key as `None`.
    #[must_use]
    pub fn ui_state(&self) -> BTreeMap<&'static str, Option<Value>> {
        self.backing
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or_else(empty_ui_state, |backing| backing.ui_state.clone())
    }

    /// Stores `value` under the allow-listed `key`, in memory first and
    /// then in the backing file. Returns `Ok(false)` without keeping
    /// anything when the workspace is ephemeral: an unsaved workspace
    /// has nowhere to put it, and the SPA keeps its own copy. A persist
    /// that fails is logged, and the in-memory value stands.
    ///
    /// Puts are serialized: the memory insert and the send to the file's
    /// actor happen under one async lock, so two overlapping puts to the
    /// same key reach the file in the order they reached memory and the
    /// next open restores what memory last held. Without it the two
    /// inserts could order one way and the two sends the other.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::UiStateKey`] for a key outside the
    /// allow-list and [`WorkspaceError::UiStateTooLarge`] for a value
    /// whose JSON text exceeds the cap; both are refused before anything
    /// changes, ephemeral or not. Persistence never fails the call.
    pub async fn put_ui_state(&self, key: &str, value: Value) -> Result<bool, WorkspaceError> {
        let key = ui_state_key(key)?;
        let json_text = value.to_string();
        check_ui_state_cap(json_text.len())?;
        let _serial = self.ui_state_puts.lock().await;
        let file = {
            let mut backing = self.backing.write().unwrap_or_else(PoisonError::into_inner);
            let Some(backing) = backing.as_mut() else {
                return Ok(false);
            };
            backing.ui_state.insert(key, Some(value));
            backing.file.clone()
        };
        if let Err(error) = file.put_ui_state(key, &json_text).await {
            tracing::warn!(
                %error,
                key,
                file = %file.path().display(),
                "ui state not persisted to the workspace file; the in-memory value stands"
            );
        }
        Ok(true)
    }
}
