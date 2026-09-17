//! The workspace file across a graceful shutdown: a quit folds the WAL
//! into `Name.pfwork` and removes the sidecar, so the folder holds
//! exactly one file at rest, and a relaunch over the same state
//! directory reopens it with every grant that was persisted before the
//! quit.

use axum::Router;
use axum::http::StatusCode;

use crate::common::{TestServer, spawn_gateway};

/// The sorted names of every entry directly inside `dir`.
// clippy.toml's allow-expect-in-tests covers #[test] functions only, not
// their helpers; failing the test by panicking with the invariant named
// is exactly what these are for.
#[expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]
fn names_in(dir: &std::path::Path) -> Vec<String> {
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

/// `POST path` on `server` with a JSON `{ "path": <target> }` body,
/// asserting success and returning the parsed answer.
#[expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]
async fn post_path(
    client: &reqwest::Client,
    server: &TestServer,
    route: &str,
    target: &std::path::Path,
) -> serde_json::Value {
    let response = client
        .post(server.http_url(route))
        .json(&serde_json::json!({ "path": target }))
        .send()
        .await
        .expect("the route answers");
    assert_eq!(response.status(), StatusCode::OK, "{route} succeeds");
    response.json().await.expect("the body is JSON")
}

#[tokio::test]
async fn a_graceful_shutdown_leaves_one_file_that_a_relaunch_reopens() {
    let gateway = spawn_gateway(Router::new()).await;
    let first = TestServer::spawn(&gateway);
    let client = reqwest::Client::new();
    let home = tempfile::TempDir::new().expect("tempdir");
    let root = tempfile::TempDir::new().expect("tempdir");
    let file = home.path().join("Name.pfwork");

    let saved = post_path(&client, &first, "/workspace/file/save_as", &file).await;
    assert_eq!(saved["name"], "Name", "save as switches to the new file");
    let granted = post_path(&client, &first, "/workspace/grant", root.path()).await;
    let granted = granted["granted"].clone();
    assert!(granted.is_string(), "the grant answers its canonical root");

    let state_dir = first.shutdown_keeping_state_dir();

    assert_eq!(
        names_in(home.path()),
        ["Name.pfwork"],
        "a graceful quit folds the WAL in and removes the sidecar"
    );

    let second = TestServer::spawn_in(&gateway, state_dir);
    let response = client
        .get(second.http_url("/workspace/file/current"))
        .send()
        .await
        .expect("the route answers");
    assert_eq!(response.status(), StatusCode::OK);
    let current: serde_json::Value = response.json().await.expect("the body is JSON");
    assert_eq!(
        current["path"],
        serde_json::json!(file),
        "the last-workspace pointer reopens the file"
    );
    assert_eq!(
        current["grants"],
        serde_json::json!([{ "path": granted, "exists": true }]),
        "the grant persisted before the quit is in the reopened file"
    );
}
