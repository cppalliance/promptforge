//! The `/workspace/file/state` route handlers: the opaque ui-state
//! bucket of the open workspace file. `GET` answers every allow-listed
//! key with its value or `null`; `PUT /{key}` stores one value and
//! answers `saved`, which is `false` while the workspace is ephemeral
//! and has nowhere to keep it.
//!
//! The body is taken raw rather than through the `Json` extractor so
//! that every refusal - an unknown key, a body over the cap, a body that
//! is not JSON - reaches the wire as the crate's own [`WorkspaceError`]
//! envelope instead of axum's plain-text rejection. The key is judged
//! first, then the body's size, then its shape, so the client is told
//! about the cheapest mistake. The raw body still passes through axum's
//! default body limit (2 MiB) before it reaches the handler; that hard
//! stop answers axum's own 413, as `PUT /workspace/file` already does
//! for oversized writes.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, put};
use serde_json::Value;

use crate::error::WorkspaceError;
use crate::workspace::Workspace;
use crate::workspace_file::{check_ui_state_cap, ui_state_key};

use super::file::SavedResponse;
use super::respond;

/// The ui-state routes, merged into the subsystem's router by the parent
/// module so they share its state and deadline tier.
pub(super) fn routes() -> axum::Router<Workspace> {
    axum::Router::new()
        .route("/workspace/file/state", get(get_state))
        .route("/workspace/file/state/{key}", put(put_state))
}

/// Reports every ui-state value the open workspace file holds, keyed by
/// its allow-listed name, `null` where nothing has been put. An
/// ephemeral workspace answers every key as `null`.
pub(crate) async fn get_state(State(workspace): State<Workspace>) -> Response {
    respond(Ok::<_, WorkspaceError>(workspace.ui_state()))
}

/// Stores the JSON body under `key` in the open workspace file. An
/// ephemeral workspace answers success with `saved: false` and writes
/// nothing; a refused key or body answers the envelope and changes
/// nothing, ephemeral or not.
pub(crate) async fn put_state(
    State(workspace): State<Workspace>,
    Path(key): Path<String>,
    body: Bytes,
) -> Response {
    respond(store(&workspace, &key, &body).await)
}

/// Validates the key, then the body's size, then its shape, and only
/// then hands the value to the workspace.
async fn store(
    workspace: &Workspace,
    key: &str,
    body: &[u8],
) -> Result<SavedResponse, WorkspaceError> {
    let key = ui_state_key(key)?;
    check_ui_state_cap(body.len())?;
    let value: Value = serde_json::from_slice(body).map_err(|_| WorkspaceError::UiStateNotJson)?;
    let saved = workspace.put_ui_state(key, value).await?;
    Ok(SavedResponse { saved })
}

#[cfg(test)]
#[path = "handlers-file-state-tests.rs"]
mod tests;
