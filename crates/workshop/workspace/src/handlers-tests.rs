use super::*;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt as _;

use crate::workspace::Workspace;

/// Collects a response body already buffered in memory.
async fn body_bytes(response: Response) -> axum::body::Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already")
}

/// A workspace with one granted tempdir, returned alongside so the
/// directory outlives the test.
fn granted_dir() -> (Workspace, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace.grant(dir.path()).expect("grant the tempdir");
    (workspace, dir)
}

/// The canonical, verbatim-prefix-free form grants are stored in.
fn simplified(path: &Path) -> std::path::PathBuf {
    dunce::simplified(&path.canonicalize().expect("canonical")).to_path_buf()
}

#[test]
fn percent_sequences_decode_before_validation() {
    assert_eq!(decode_path_param("%2e%2e/x"), "../x");
    assert_eq!(decode_path_param("plain.txt"), "plain.txt");
    // An invalid sequence is not an encoding; the literal survives.
    assert_eq!(decode_path_param("100%.txt"), "100%.txt");
}

/// A double-encoded traversal (`%252e%252e` in the raw query) survives
/// the query layer's single decode as `%2e%2e`; the handler's explicit
/// decode must still reveal it to the lexical `..` check.
#[tokio::test]
async fn an_encoded_traversal_in_the_query_is_rejected() {
    let (workspace, _dir) = granted_dir();
    let router = routes(workspace);
    for uri in [
        "/workspace/file?path=%252e%252e%2Fsecret.txt",
        "/workspace/tree?path=%252e%252e",
    ] {
        let request = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router
            .clone()
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "for {uri}");
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("the envelope is JSON");
        assert_eq!(
            json["error"]["code"], "forbidden_component",
            "the traversal must be caught lexically, not by a lookup miss: {uri}"
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
