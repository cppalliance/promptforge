//! Workspace handler tests: query paths decoded exactly once, traversal refusals, and revokes.

use super::*;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt as _;

use crate::test_support::{body_bytes, granted_dir, simplified};
use crate::workspace::Workspace;

/// Percent-encodes a path once for a query string, as the UI does.
fn query_encoded(path: &Path) -> String {
    percent_encoding::utf8_percent_encode(
        &path.to_string_lossy(),
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string()
}

/// Sends a bodiless `GET` through a clone of `router`.
async fn get(router: &axum::Router, uri: &str) -> Response {
    let request = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("static request parts are valid");
    router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible")
}

/// A granted root literally named `b%42` beside a sibling holding a file
/// outside the grant, returned with the tempdir so it outlives the test.
fn granted_percent_root() -> (Workspace, std::path::PathBuf, tempfile::TempDir) {
    let parent = tempfile::TempDir::new().expect("tempdir");
    let root = parent.path().join("b%42");
    std::fs::create_dir(&root).expect("create the percent-named root");
    std::fs::write(parent.path().join("secret.txt"), "secret contents")
        .expect("seed the file outside the grant");
    let workspace = Workspace::new();
    workspace
        .grant(&root)
        .expect("grant the percent-named root");
    (workspace, simplified(&root), parent)
}

/// A name that still looks percent-encoded is a literal name: PUT writes
/// it from the undecoded JSON body, and both GET routes find it again
/// from a query encoded exactly once.
#[tokio::test]
async fn a_literal_percent_name_round_trips_through_put_and_get() {
    let (workspace, root, _parent) = granted_percent_root();
    let router = routes(workspace);
    let file = root.join("a%41.txt");

    let body = serde_json::json!({ "path": file, "text": "literal" }).to_string();
    let put = Request::builder()
        .method("PUT")
        .uri("/workspace/file")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("static request parts are valid");
    let response = router
        .clone()
        .oneshot(put)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(file.is_file(), "the PUT wrote the literal name");

    let response = get(
        &router,
        &format!("/workspace/file?path={}", query_encoded(&file)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON");
    assert_eq!(json["path"], serde_json::json!(file));
    assert_eq!(json["text"], "literal");

    let response = get(
        &router,
        &format!("/workspace/tree?path={}", query_encoded(&root)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON");
    assert_eq!(json["path"], serde_json::json!(root));
    assert_eq!(json["entries"][0]["name"], "a%41.txt");
}

/// A double-encoded traversal (`%252e%252e` in the raw query) survives
/// the query layer's single decode as `%2e%2e`, a literal name. No such
/// entry exists inside the grant, so the request fails, and neither the
/// file nor the listing the decoded `..` would reach comes back.
#[tokio::test]
async fn a_double_encoded_traversal_returns_nothing_outside_the_grant() {
    let (workspace, root, _parent) = granted_percent_root();
    let router = routes(workspace);
    let root = query_encoded(&root);
    for uri in [
        format!("/workspace/file?path={root}%2F%252e%252e%2Fsecret.txt"),
        format!("/workspace/tree?path={root}%2F%252e%252e"),
    ] {
        let response = get(&router, &uri).await;
        assert_ne!(response.status(), StatusCode::OK, "for {uri}");
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("the envelope is JSON");
        assert!(json.get("error").is_some(), "an error envelope for {uri}");
        assert!(
            !String::from_utf8_lossy(&body).contains("secret"),
            "nothing outside the grant comes back for {uri}"
        );
    }
}

/// Builds a `POST /workspace/revoke` request with a raw JSON body.
fn revoke_request(body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/workspace/revoke")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("static request parts are valid")
}

#[tokio::test]
async fn a_revoke_over_http_removes_the_root() {
    let (workspace, dir) = granted_dir();
    let router = routes(workspace.clone());
    let root = simplified(dir.path());
    let body = serde_json::json!({ "path": root }).to_string();
    let response = router
        .oneshot(revoke_request(body))
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body_bytes(response).await;
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("the body is JSON");
    assert_eq!(json["revoked"], serde_json::json!(root));
    assert_eq!(workspace.granted_roots(), Vec::<std::path::PathBuf>::new());
}

#[tokio::test]
async fn an_unknown_root_revoke_answers_not_found() {
    let (workspace, _dir) = granted_dir();
    let outside = tempfile::TempDir::new().expect("outside tempdir");
    let router = routes(workspace);
    let body = serde_json::json!({ "path": outside.path() }).to_string();
    let response = router
        .oneshot(revoke_request(body))
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = body_bytes(response).await;
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("the body is JSON");
    assert_eq!(json["error"]["code"], "not_granted");
}

#[tokio::test]
async fn a_malformed_revoke_body_answers_bad_request() {
    let (workspace, _dir) = granted_dir();
    let router = routes(workspace);
    let response = router
        .oneshot(revoke_request("{".to_owned()))
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
