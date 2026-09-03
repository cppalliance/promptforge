//! Routes for confined workspace filesystem access.

use axum::Router;
use axum::routing::{get, post};

use crate::workspace::{self, Workspace};

/// The workspace routes, narrowed to the [`Workspace`] service - the only
/// state their handlers use.
pub(crate) fn routes(state: Workspace) -> Router {
    Router::new()
        .route("/workspace/tree", get(workspace::tree))
        .route(
            "/workspace/file",
            get(workspace::read_file).put(workspace::write_file),
        )
        .route("/workspace/grant", post(workspace::grant))
        .route("/workspace/revoke", post(workspace::revoke))
        .with_state(state)
}
