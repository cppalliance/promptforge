//! The `/workspace/file/*` routes over HTTP: the happy path of every
//! endpoint against tempdir files, the refusals the client can act on
//! (alien file, missing path, taken path), and the ephemeral no-op of
//! the window-state write.

use super::*;

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

use crate::handlers::routes;
use crate::workspace_file::open_database;

/// Collects a response body already buffered in memory and parses it.
async fn json_body(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already");
    serde_json::from_slice(&bytes).expect("the body is JSON")
}

/// Builds a request with a raw JSON body.
fn json_request(method: &str, uri: &str, body: String) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("static request parts are valid")
}

/// Builds a `GET /workspace/file/current` request.
fn current_request() -> Request<Body> {
    Request::builder()
        .uri("/workspace/file/current")
        .body(Body::empty())
        .expect("static request parts are valid")
}

/// A JSON body naming `path`.
fn path_body(path: &Path) -> String {
    serde_json::json!({ "path": path }).to_string()
}

/// Sends `request` through a fresh clone of the workspace's router.
async fn send(workspace: &Workspace, request: Request<Body>) -> Response {
    routes(workspace.clone())
        .oneshot(request)
        .await
        .expect("the router is infallible")
}

/// The canonical, verbatim-prefix-free form grants are stored in.
fn simplified(path: &Path) -> PathBuf {
    dunce::simplified(&path.canonicalize().expect("canonical")).to_path_buf()
}

/// A window geometry distinguished by `width`, as the desktop app would send it.
fn window_body(width: u32) -> String {
    serde_json::json!({
        "width": width,
        "height": 700,
        "x": 5,
        "y": 6,
        "maximized": false,
    })
    .to_string()
}

/// The sorted names of every entry directly inside `dir`.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("the directory lists")
        .map(|entry| {
            entry
                .expect("entry reads")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn current_on_an_ephemeral_workspace_reports_no_file_and_its_grants() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    let root = workspace.grant(dir.path()).expect("grant the tempdir");

    let response = send(&workspace, current_request()).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["path"], serde_json::Value::Null);
    assert_eq!(json["name"], "Untitled");
    assert_eq!(json["window_state"], serde_json::Value::Null);
    assert_eq!(
        json["grants"],
        serde_json::json!([{ "path": root, "exists": true }])
    );
}

#[tokio::test]
async fn save_as_then_current_shows_the_new_path_and_name() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    let root = workspace.grant(dir.path()).expect("grant the tempdir");

    let response = send(
        &workspace,
        json_request("POST", "/workspace/file/save-as", path_body(&file_path)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["path"], file_path.to_string_lossy().as_ref());
    assert_eq!(json["name"], "mine");
    assert_eq!(
        json["grants"],
        serde_json::json!([{ "path": root, "exists": true }])
    );

    let current = json_body(send(&workspace, current_request()).await).await;
    assert_eq!(current["path"], file_path.to_string_lossy().as_ref());
    assert_eq!(current["name"], "mine");
    workspace.close_backing_for_test().await;
    assert_eq!(
        names_in(home.path()),
        ["mine.pfwork"],
        "save as creates exactly the file"
    );
}

#[tokio::test]
async fn open_replaces_the_grants_with_the_files() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("theirs.pfwork");
    let theirs = tempfile::TempDir::new().expect("tempdir");
    let mine = tempfile::TempDir::new().expect("tempdir");
    let author = Workspace::new();
    author.grant(theirs.path()).expect("grant their root");
    author.save_as(&file_path).await.expect("save as creates");
    author.close_backing_for_test().await;

    let workspace = Workspace::new();
    workspace.grant(mine.path()).expect("grant my root");
    let response = send(
        &workspace,
        json_request("POST", "/workspace/file/open", path_body(&file_path)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["path"], file_path.to_string_lossy().as_ref());
    assert_eq!(json["name"], "theirs");
    assert_eq!(
        json["grants"],
        serde_json::json!([{ "path": simplified(theirs.path()), "exists": true }]),
        "open is wholesale: the prior grant is gone and the file's grant is in"
    );
    assert_eq!(
        workspace.granted_roots(),
        vec![simplified(theirs.path())],
        "the router shares the workspace's grant set"
    );
}

#[tokio::test]
async fn an_alien_database_is_refused_and_current_still_shows_the_prior_grants() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let alien_path = home.path().join("alien.pfwork");
    let mine = tempfile::TempDir::new().expect("tempdir");
    {
        let conn = open_database(&alien_path)
            .await
            .expect("the alien database opens");
        conn.execute_batch("CREATE TABLE notes (body TEXT NOT NULL);")
            .await
            .expect("a foreign table creates");
    }
    let workspace = Workspace::new();
    let root = workspace.grant(mine.path()).expect("grant mine");
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");

    let response = send(
        &workspace,
        json_request("POST", "/workspace/file/open", path_body(&alien_path)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "workspace_file_refused");
    let message = json["error"]["message"]
        .as_str()
        .expect("the envelope includes a message");
    assert!(
        message.contains("is not a promptforge workspace file"),
        "the refusal names what was required: {message}"
    );

    let current = json_body(send(&workspace, current_request()).await).await;
    assert_eq!(
        current["path"],
        file_path.to_string_lossy().as_ref(),
        "the backing stands"
    );
    assert_eq!(
        current["grants"],
        serde_json::json!([{ "path": root, "exists": true }]),
        "the grants stand"
    );
}

#[tokio::test]
async fn opening_a_missing_path_answers_not_found() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();

    let response = send(
        &workspace,
        json_request(
            "POST",
            "/workspace/file/open",
            path_body(&home.path().join("nowhere.pfwork")),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "not_found");
    let current = json_body(send(&workspace, current_request()).await).await;
    assert_eq!(current["path"], serde_json::Value::Null);
}

#[tokio::test]
async fn save_as_onto_a_taken_path_answers_conflict() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let taken = home.path().join("taken.pfwork");
    std::fs::write(&taken, b"already here").expect("placeholder writes");
    let workspace = Workspace::new();

    let response = send(
        &workspace,
        json_request("POST", "/workspace/file/save-as", path_body(&taken)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "workspace_file_taken");
    assert_eq!(
        std::fs::read(&taken).expect("placeholder reads"),
        b"already here",
        "save as never overwrites"
    );
}

#[tokio::test]
async fn put_window_state_while_ephemeral_is_a_no_op() {
    let workspace = Workspace::new();

    let response = send(
        &workspace,
        json_request("PUT", "/workspace/file/window-state", window_body(800)),
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an ephemeral save is not a failure"
    );
    let json = json_body(response).await;
    assert_eq!(
        json["saved"], false,
        "the caller is told nothing was written"
    );
    let current = json_body(send(&workspace, current_request()).await).await;
    assert_eq!(current["window_state"], serde_json::Value::Null);
}

#[tokio::test]
async fn put_window_state_on_a_file_backed_workspace_persists() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let workspace = Workspace::new();
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");

    let response = send(
        &workspace,
        json_request("PUT", "/workspace/file/window-state", window_body(1111)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["saved"], true);
    let current = json_body(send(&workspace, current_request()).await).await;
    assert_eq!(
        current["window_state"],
        serde_json::json!({
            "width": 1111,
            "height": 700,
            "x": 5,
            "y": 6,
            "maximized": false,
        })
    );
}

#[tokio::test]
async fn duplicate_switches_current_to_the_copy() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let original_path = home.path().join("original.pfwork");
    let copy_path = home.path().join("copy.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    let root = workspace.grant(dir.path()).expect("grant the tempdir");
    workspace
        .save_as(&original_path)
        .await
        .expect("save as creates");

    let response = send(
        &workspace,
        json_request("POST", "/workspace/file/duplicate", path_body(&copy_path)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["path"], copy_path.to_string_lossy().as_ref());
    assert_eq!(
        json["name"], "original",
        "the copy keeps the original's display name"
    );
    assert_eq!(
        json["grants"],
        serde_json::json!([{ "path": root, "exists": true }])
    );
    let current = json_body(send(&workspace, current_request()).await).await;
    assert_eq!(
        current["path"],
        copy_path.to_string_lossy().as_ref(),
        "duplicate switches to the copy"
    );
    workspace.close_backing_for_test().await;
    assert_eq!(
        names_in(home.path()),
        ["copy.pfwork", "original.pfwork"],
        "both files exist and neither left a sidecar once closed"
    );
}

#[tokio::test]
async fn a_malformed_file_body_answers_bad_request() {
    let workspace = Workspace::new();
    for uri in [
        "/workspace/file/open",
        "/workspace/file/save-as",
        "/workspace/file/duplicate",
    ] {
        let response = send(&workspace, json_request("POST", uri, "{".to_owned())).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "for {uri}");
    }
    let response = send(
        &workspace,
        json_request("PUT", "/workspace/file/window-state", "{".to_owned()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
