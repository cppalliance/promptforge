//! The `/workspace/*` route handlers: query and body DTOs, the path
//! decoding that defangs double-encoded traversal, and the router
//! constructor the subsystem registers into the registry.

use std::path::Path;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use workshop_support::{DEFAULT_DEADLINE, with_deadline};

use crate::error::WorkspaceError;
use crate::workspace::Workspace;

/// The workspace routes, narrowed to the [`Workspace`] service - the only
/// state their handlers use. Every route carries the default deadline
/// tier.
pub fn routes(state: Workspace) -> axum::Router {
    with_deadline(
        axum::Router::new()
            .route("/workspace/tree", get(tree))
            .route("/workspace/file", get(read_file).put(write_file))
            .route("/workspace/grant", post(grant))
            .route("/workspace/revoke", post(revoke))
            .with_state(state),
        DEFAULT_DEADLINE,
    )
}

/// The query string of `GET /workspace/tree`.
#[derive(Debug, Deserialize)]
pub(crate) struct TreeQuery {
    /// The directory to list; absent or empty lists the granted roots.
    path: Option<String>,
}

/// The query string of `GET /workspace/file`.
#[derive(Debug, Deserialize)]
pub(crate) struct FileQuery {
    /// The file to read.
    path: String,
}

/// The JSON body of `PUT /workspace/file`.
#[derive(Debug, Deserialize)]
pub(crate) struct WriteRequest {
    /// The file to write.
    path: String,
    /// The new UTF-8 contents.
    text: String,
    /// The conflict token the writer last read; required to match when
    /// the file already exists.
    expected_token: Option<String>,
}

/// The JSON body of `POST /workspace/grant`.
#[derive(Debug, Deserialize)]
pub(crate) struct GrantRequest {
    /// The dropped path: a folder grants itself, a file grants its parent.
    path: String,
}

/// The JSON body of a successful grant.
#[derive(Debug, Serialize)]
pub(crate) struct GrantResponse {
    /// The root that was registered.
    granted: std::path::PathBuf,
}

/// The JSON body of `POST /workspace/revoke`.
#[derive(Debug, Deserialize)]
pub(crate) struct RevokeRequest {
    /// The granted root to remove, as listed by the roots tree.
    path: String,
}

/// The JSON body of a successful revoke.
#[derive(Debug, Serialize)]
pub(crate) struct RevokeResponse {
    /// The root that was removed.
    revoked: std::path::PathBuf,
}

/// Percent-decodes a workspace path parameter before validation. The query
/// layer already decoded once, so any surviving `%XX` sequence is a second
/// encoding layer - decoding it here means an encoded traversal (`%2e%2e`)
/// reaches the lexical `..` check as a literal `..` however the client
/// encoded it. Invalid sequences pass through unchanged.
fn decode_path_param(raw: &str) -> String {
    percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned()
}

/// Lists one level of a workspace directory, or the granted roots when the
/// query carries no path.
pub(crate) async fn tree(
    State(workspace): State<Workspace>,
    Query(query): Query<TreeQuery>,
) -> Response {
    let path = query.path.as_deref().map(decode_path_param);
    respond(workspace.tree(path.as_deref().map(Path::new)))
}

/// Reads a confined UTF-8 text file with its metadata.
pub(crate) async fn read_file(
    State(workspace): State<Workspace>,
    Query(query): Query<FileQuery>,
) -> Response {
    let path = decode_path_param(&query.path);
    respond(workspace.read_file(Path::new(&path)))
}

/// Writes a confined file after path, size, and conflict-token validation.
pub(crate) async fn write_file(
    State(workspace): State<Workspace>,
    Json(body): Json<WriteRequest>,
) -> Response {
    respond(workspace.write_file(
        Path::new(&body.path),
        &body.text,
        body.expected_token.as_deref(),
    ))
}

/// Registers a dropped path as a granted root, mirrored into the open
/// workspace file when there is one.
pub(crate) async fn grant(
    State(workspace): State<Workspace>,
    Json(body): Json<GrantRequest>,
) -> Response {
    respond(
        workspace
            .grant_and_persist(Path::new(&body.path))
            .await
            .map(|granted| GrantResponse { granted }),
    )
}

/// Removes a granted root, mirrored into the open workspace file when
/// there is one; paths under it fail their next operation.
pub(crate) async fn revoke(
    State(workspace): State<Workspace>,
    Json(body): Json<RevokeRequest>,
) -> Response {
    respond(
        workspace
            .revoke_and_persist(Path::new(&body.path))
            .await
            .map(|revoked| RevokeResponse { revoked }),
    )
}

/// Renders a workspace result as JSON, routing failures through the
/// [`WorkspaceError`] wire envelope.
fn respond<T: Serialize>(result: Result<T, WorkspaceError>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
#[path = "handlers-tests.rs"]
mod tests;
