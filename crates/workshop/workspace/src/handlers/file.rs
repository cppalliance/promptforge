//! The `/workspace/file/*` route handlers: the workspace as a document.
//! What is open, opening another file, saving as, duplicating, and the
//! window geometry the desktop app keeps in it. Every mutation answers with
//! the workspace as it stands afterwards, so the client never needs a
//! second round trip to learn what it switched to.
//!
//! Failures reach the wire through [`WorkspaceError`]'s envelope: a
//! refused file (alien or unsupported version) is the client's mistake
//! and includes the refusal's required-versus-actual text, a missing
//! path is the ordinary not-found, a taken path a conflict. A refusal
//! leaves the grants and the backing as they were.

use std::path::Path;

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};

use crate::error::WorkspaceError;
use crate::workspace::{GrantEntry, Workspace, WorkspaceSummary};
use crate::workspace_file::WindowState;

use super::respond;

/// The workspace-file routes, merged into the subsystem's router by the
/// parent module so they share its state and deadline tier.
pub(super) fn routes() -> axum::Router<Workspace> {
    axum::Router::new()
        .route("/workspace/file/current", get(current_file))
        .route("/workspace/file/open", post(open_file))
        .route("/workspace/file/save_as", post(save_as_file))
        .route("/workspace/file/duplicate", post(duplicate_file))
        .route("/workspace/file/window-state", put(put_window_state))
}

/// The JSON body of `GET /workspace/file/current` and of every
/// successful switch: the workspace as it stands.
#[derive(Debug, Serialize)]
pub(crate) struct WorkspaceFileResponse {
    /// The backing file; `null` while the workspace is ephemeral.
    path: Option<String>,
    /// The display name: the file's own, or `Untitled` while ephemeral.
    name: String,
    /// The granted roots in canonical order.
    grants: Vec<GrantEntry>,
    /// The saved window geometry; `null` while ephemeral or never saved.
    window_state: Option<WindowState>,
}

impl From<WorkspaceSummary> for WorkspaceFileResponse {
    fn from(summary: WorkspaceSummary) -> Self {
        Self {
            path: summary.path.map(|path| path.to_string_lossy().into_owned()),
            name: summary.name,
            grants: summary.grants,
            window_state: summary.window_state,
        }
    }
}

/// The JSON body of `POST /workspace/file/{open,save_as,duplicate}`.
#[derive(Debug, Deserialize)]
pub(crate) struct FilePathRequest {
    /// The workspace file to open, or the path to create the new file at.
    path: String,
}

/// The JSON body of a `PUT /workspace/file/window-state` or
/// `PUT /workspace/file/state/{key}` answer.
#[derive(Debug, Serialize)]
pub(crate) struct SavedResponse {
    /// Whether the value was written; `false` when the workspace is
    /// ephemeral and has nowhere to keep it.
    pub(super) saved: bool,
}

/// Reports the workspace as it stands: its file, name, grants, and
/// saved window geometry.
pub(crate) async fn current_file(State(workspace): State<Workspace>) -> Response {
    respond(Ok::<_, WorkspaceError>(WorkspaceFileResponse::from(
        workspace.current().await,
    )))
}

/// Opens a workspace file, replacing every grant with its contents, and
/// answers with the workspace as opened. A refused or missing file
/// changes nothing.
pub(crate) async fn open_file(
    State(workspace): State<Workspace>,
    Json(body): Json<FilePathRequest>,
) -> Response {
    let result = workspace.open_file(Path::new(&body.path)).await;
    after_switch(&workspace, result).await
}

/// Creates a new workspace file holding the current grants and switches
/// to it; the previous file, if any, stays where it is.
pub(crate) async fn save_as_file(
    State(workspace): State<Workspace>,
    Json(body): Json<FilePathRequest>,
) -> Response {
    let result = workspace.save_as(Path::new(&body.path)).await;
    after_switch(&workspace, result).await
}

/// Copies the current workspace file and its siblings to a new path and
/// switches to the copy.
pub(crate) async fn duplicate_file(
    State(workspace): State<Workspace>,
    Json(body): Json<FilePathRequest>,
) -> Response {
    let result = workspace.duplicate(Path::new(&body.path)).await;
    after_switch(&workspace, result).await
}

/// Saves the desktop app's window geometry into the open workspace file. An
/// ephemeral workspace answers success with `saved: false` and writes
/// nothing.
pub(crate) async fn put_window_state(
    State(workspace): State<Workspace>,
    Json(state): Json<WindowState>,
) -> Response {
    respond(
        workspace
            .put_window_state(state)
            .await
            .map(|saved| SavedResponse { saved }),
    )
}

/// Renders the outcome of a switch: the workspace as it now stands on
/// success, the failure's envelope otherwise.
async fn after_switch(workspace: &Workspace, result: Result<(), WorkspaceError>) -> Response {
    match result {
        Ok(()) => respond(Ok::<_, WorkspaceError>(WorkspaceFileResponse::from(
            workspace.current().await,
        ))),
        Err(error) => respond(Err::<WorkspaceFileResponse, _>(error)),
    }
}

#[cfg(test)]
mod tests;
