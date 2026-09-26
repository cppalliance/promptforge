//! The `/workspace/file/state` routes over HTTP: three nulls before any
//! put, a put-then-get round trip per key that also survives closing and
//! reopening the file, the three refusals (unknown key, over-cap body,
//! non-JSON body) each answering the envelope and leaving the state
//! untouched, and the ephemeral put that says `saved: false`.

use super::*;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;
use workshop_support::STATE_BUCKET_VALUE_CAP;

use crate::handlers::routes;
use crate::test_support::json_body;

/// Builds a `GET /workspace/file/state` request.
fn get_request() -> Request<Body> {
    Request::builder()
        .uri("/workspace/file/state")
        .body(Body::empty())
        .expect("static request parts are valid")
}

/// Builds a `PUT /workspace/file/state/{key}` request with a raw body.
fn put_request(key: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/workspace/file/state/{key}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(body.into())
        .expect("static request parts are valid")
}

/// Sends `request` through a fresh clone of the workspace's router.
async fn send(workspace: &Workspace, request: Request<Body>) -> Response {
    routes(workspace.clone())
        .oneshot(request)
        .await
        .expect("the router is infallible")
}

/// The GET body of `workspace` as it stands.
async fn state_of(workspace: &Workspace) -> serde_json::Value {
    let response = send(workspace, get_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

/// What every key reads as before any put.
fn all_null() -> serde_json::Value {
    serde_json::json!({ "layout": null, "tree": null, "closed_editors": null })
}

/// One distinct value per allow-listed key.
fn sample_values() -> [(&'static str, serde_json::Value); 3] {
    [
        (
            "layout",
            serde_json::json!({ "version": 3, "zones": { "tree": 240 } }),
        ),
        (
            "tree",
            serde_json::json!({ "expanded": ["C:\\projects\\zebra"] }),
        ),
        (
            "closed_editors",
            serde_json::json!({ "paths": ["C:\\projects\\zebra\\a.md"] }),
        ),
    ]
}

/// A workspace backed by a fresh file under `home`.
async fn file_backed(home: &tempfile::TempDir) -> (Workspace, std::path::PathBuf) {
    let path = home.path().join("mine.pfwork");
    let workspace = Workspace::new();
    workspace.save_as(&path).await.expect("save as creates");
    (workspace, path)
}

#[tokio::test]
async fn get_with_no_file_returns_three_nulls() {
    let workspace = Workspace::new();

    assert_eq!(
        state_of(&workspace).await,
        all_null(),
        "every allow-listed key is present and null while ephemeral"
    );
}

#[tokio::test]
async fn put_then_get_round_trips_each_key_and_survives_reopen() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let (workspace, path) = file_backed(&home).await;
    assert_eq!(state_of(&workspace).await, all_null());

    for (key, value) in sample_values() {
        let response = send(&workspace, put_request(key, value.to_string())).await;
        assert_eq!(response.status(), StatusCode::OK, "for {key}");
        assert_eq!(
            json_body(response).await,
            serde_json::json!({ "saved": true })
        );
        assert_eq!(
            state_of(&workspace).await[key],
            value,
            "{key} reads back exactly as put"
        );
    }
    let expected: serde_json::Value = sample_values()
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect::<serde_json::Map<_, _>>()
        .into();
    assert_eq!(state_of(&workspace).await, expected);
    workspace.close_backing_for_test().await;

    let reopened = Workspace::new();
    reopened.open_file(&path).await.expect("the file reopens");
    assert_eq!(
        state_of(&reopened).await,
        expected,
        "the route wrote through to the file, not only to memory"
    );
}

#[tokio::test]
async fn a_put_replaces_the_earlier_value() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let (workspace, _path) = file_backed(&home).await;
    let first = send(
        &workspace,
        put_request("tree", r#"{"expanded":["C:\\one"]}"#),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK, "the first put must land");
    assert_eq!(json_body(first).await, serde_json::json!({ "saved": true }));
    assert_eq!(
        state_of(&workspace).await["tree"],
        serde_json::json!({ "expanded": ["C:\\one"] }),
        "the value being replaced was actually stored"
    );

    let response = send(&workspace, put_request("tree", r#"{"expanded":[]}"#)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state_of(&workspace).await["tree"],
        serde_json::json!({ "expanded": [] })
    );
}

#[tokio::test]
async fn an_unknown_key_is_refused_and_the_state_stands() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let (workspace, _path) = file_backed(&home).await;

    let response = send(&workspace, put_request("scroll", "{}")).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "ui_state_key");
    let message = json["error"]["message"]
        .as_str()
        .expect("the envelope includes a message");
    assert!(
        message.contains("layout, tree, closed_editors"),
        "the refusal names what is allowed: {message}"
    );
    assert_eq!(state_of(&workspace).await, all_null());
}

#[tokio::test]
async fn an_over_cap_body_is_refused_and_the_state_stands() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let (workspace, _path) = file_backed(&home).await;
    let body = format!("\"{}\"", "x".repeat(STATE_BUCKET_VALUE_CAP - 1));
    assert_eq!(
        body.len(),
        STATE_BUCKET_VALUE_CAP + 1,
        "one byte past the cap"
    );

    let response = send(&workspace, put_request("layout", body)).await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "ui_state_too_large");
    let message = json["error"]["message"]
        .as_str()
        .expect("the envelope includes a message");
    assert!(
        message.contains(&format!("{} bytes", STATE_BUCKET_VALUE_CAP + 1))
            && message.contains(&format!("{STATE_BUCKET_VALUE_CAP} bytes")),
        "the refusal names actual and cap: {message}"
    );
    assert_eq!(state_of(&workspace).await, all_null());
}

#[tokio::test]
async fn a_body_exactly_at_the_cap_is_accepted() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let (workspace, _path) = file_backed(&home).await;
    let body = format!("\"{}\"", "x".repeat(STATE_BUCKET_VALUE_CAP - 2));
    assert_eq!(body.len(), STATE_BUCKET_VALUE_CAP);

    let response = send(&workspace, put_request("layout", body.clone())).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state_of(&workspace).await["layout"],
        serde_json::Value::String("x".repeat(STATE_BUCKET_VALUE_CAP - 2))
    );
}

#[tokio::test]
async fn a_non_json_body_is_refused_and_the_state_stands() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let (workspace, _path) = file_backed(&home).await;

    for body in ["{", "", "not json at all"] {
        let response = send(&workspace, put_request("layout", body)).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "for body {body:?}"
        );
        let json = json_body(response).await;
        assert_eq!(json["error"]["code"], "ui_state_not_json", "for {body:?}");
    }
    let response = send(
        &workspace,
        put_request("layout", Body::from(vec![0xff, 0xfe, b'"'])),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "bytes that are not UTF-8 are not JSON either"
    );
    assert_eq!(
        json_body(response).await["error"]["code"],
        "ui_state_not_json"
    );
    assert_eq!(state_of(&workspace).await, all_null());
}

#[tokio::test]
async fn an_unknown_key_is_refused_before_the_body_is_judged() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let (workspace, _path) = file_backed(&home).await;

    let response = send(&workspace, put_request("scroll", "{")).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "ui_state_key",
        "the key refusal wins over the body refusal"
    );
}

#[tokio::test]
async fn put_while_ephemeral_answers_saved_false_and_keeps_nothing() {
    let workspace = Workspace::new();

    let response = send(&workspace, put_request("layout", r#"{"version":3}"#)).await;

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an ephemeral put is not a failure"
    );
    assert_eq!(
        json_body(response).await,
        serde_json::json!({ "saved": false }),
        "the caller is told nothing was written"
    );
    assert_eq!(state_of(&workspace).await, all_null());
}

#[tokio::test]
async fn a_refused_put_while_ephemeral_is_still_refused() {
    let workspace = Workspace::new();

    let response = send(&workspace, put_request("scroll", "{}")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(response).await["error"]["code"], "ui_state_key");

    let response = send(&workspace, put_request("layout", "{")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "ui_state_not_json"
    );
}
