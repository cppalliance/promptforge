//! The workspace-level view of the opaque ui-state values: an in-memory
//! map on the backing, read from the file at open and updated on every
//! accepted put, mirrored into the file through the actor. Memory is
//! the source of truth, as it is for grants and window state: a persist
//! that fails is logged and the value stands.

use std::collections::BTreeMap;
use std::sync::PoisonError;

use serde_json::Value;
use workshop_support::StateBucketValue;

use crate::Workspace;
use crate::workspace_file::empty_ui_state;

impl Workspace {
    /// The ui-state values as memory holds them: every allow-listed key,
    /// each `None` until put or read from the file at open. An ephemeral
    /// workspace has nowhere to keep them and reports every key as `None`.
    #[must_use]
    pub(crate) fn ui_state(&self) -> BTreeMap<&'static str, Option<Value>> {
        self.backing
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or_else(empty_ui_state, |backing| backing.ui_state.clone())
    }

    /// Stores the validated `put` under its key, in memory first and
    /// then in the backing file. Returns `false` without keeping
    /// anything when the workspace is ephemeral: an unsaved workspace
    /// has nowhere to put it, and the SPA keeps its own copy. A persist
    /// that fails is logged, and the in-memory value stands.
    ///
    /// Puts are serialized: the memory insert and the send to the file's
    /// actor happen under one async lock, so two overlapping puts to the
    /// same key reach the file in the order they reached memory and the
    /// next open restores what memory last held. Without it the two
    /// inserts could order one way and the two sends the other.
    pub(crate) async fn put_ui_state(&self, put: StateBucketValue) -> bool {
        let _serial = self.ui_state_puts.lock().await;
        let file = {
            let mut backing = self.backing.write().unwrap_or_else(PoisonError::into_inner);
            let Some(backing) = backing.as_mut() else {
                return false;
            };
            backing
                .ui_state
                .insert(put.key(), Some(put.value().clone()));
            backing.file.clone()
        };
        if let Err(error) = file.put_ui_state(&put).await {
            tracing::warn!(
                %error,
                key = put.key(),
                file = %file.path().display(),
                "ui state not persisted to the workspace file; the in-memory value stands"
            );
        }
        true
    }
}
