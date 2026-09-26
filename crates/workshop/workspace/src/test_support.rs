//! Helpers the crate's unit tests share: a workspace over one granted
//! tempdir, the canonical form grants are stored in, and readers for a
//! route's buffered response body.

use std::path::{Path, PathBuf};

use axum::response::Response;

use crate::workspace::Workspace;

/// A workspace with one granted tempdir, returned alongside so the
/// directory outlives the test.
pub(crate) fn granted_dir() -> (Workspace, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace.grant(dir.path()).expect("grant the tempdir");
    (workspace, dir)
}

/// The canonical, verbatim-prefix-free form grants are stored in.
pub(crate) fn simplified(path: &Path) -> PathBuf {
    dunce::simplified(&path.canonicalize().expect("canonical")).to_path_buf()
}

/// Collects a response body already buffered in memory.
pub(crate) async fn body_bytes(response: Response) -> axum::body::Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already")
}

/// Collects a response body already buffered in memory and parses it.
pub(crate) async fn json_body(response: Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON")
}
