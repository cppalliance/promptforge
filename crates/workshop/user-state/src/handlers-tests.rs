//! The `/user/state` routes over HTTP: four nulls before any put, a
//! put-then-get round trip per key that also survives a fresh store over
//! the same directory, the three refusals (unknown key, over-cap body,
//! non-JSON body) each answering the envelope and leaving the state
//! untouched, and the write failure answering the server-error envelope
//! while the value stands in memory.

use super::*;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;
use workshop_support::STATE_BUCKET_VALUE_CAP;

use crate::store::USER_STATE_FILE;

/// Collects a response body already buffered in memory and parses it.
async fn json_body(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already");
    serde_json::from_slice(&bytes).expect("the body is JSON")
}

/// Builds a `GET /user/state` request.
fn get_request() -> Request<Body> {
    Request::builder()
        .uri("/user/state")
        .body(Body::empty())
        .expect("static request parts are valid")
}

/// Builds a `PUT /user/state/{key}` request with a raw body.
fn put_request(key: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/user/state/{key}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(body.into())
        .expect("static request parts are valid")
}

/// Sends `request` through a fresh router over `store`.
async fn send(store: &Arc<UserStateStore>, request: Request<Body>) -> Response {
    routes(Arc::clone(store))
        .oneshot(request)
        .await
        .expect("the router is infallible")
}

/// The GET body of `store` as it stands.
async fn state_of(store: &Arc<UserStateStore>) -> serde_json::Value {
    let response = send(store, get_request()).await;
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

/// A store over a fresh temp directory.
fn fresh_store(dir: &tempfile::TempDir) -> Arc<UserStateStore> {
    Arc::new(UserStateStore::new(dir.path()))
}

/// What every key reads as before any put.
fn all_null() -> serde_json::Value {
    serde_json::json!({
        "editor_settings": null,
        "zoom": null,
        "recent_files": null,
        "commands_history": null,
    })
}

/// One distinct value per allow-listed key.
fn sample_values() -> [(&'static str, serde_json::Value); 4] {
    [
        (
            "editor_settings",
            serde_json::json!({ "wordWrap": "on", "renderWhitespace": "none" }),
        ),
        ("zoom", serde_json::json!(1.25)),
        (
            "recent_files",
            serde_json::json!(["C:\\projects\\zebra\\a.md", "C:\\projects\\zebra\\b.md"]),
        ),
        (
            "commands_history",
            serde_json::json!(["workbench.action.files.save"]),
        ),
    ]
}

#[tokio::test]
async fn get_on_an_empty_store_returns_four_nulls_and_creates_no_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = fresh_store(&dir);

    assert_eq!(
        state_of(&store).await,
        all_null(),
        "every allow-listed key is present and null before any put"
    );
    assert!(
        !dir.path().join(USER_STATE_FILE).exists(),
        "a read never creates the state file"
    );
}

#[tokio::test]
async fn put_then_get_round_trips_each_key_and_survives_a_fresh_store() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = fresh_store(&dir);

    for (key, value) in sample_values() {
        let response = send(&store, put_request(key, value.to_string())).await;
        assert_eq!(response.status(), StatusCode::OK, "for {key}");
        assert_eq!(
            json_body(response).await,
            serde_json::json!({ "saved": true })
        );
        assert_eq!(
            state_of(&store).await[key],
            value,
            "{key} reads back exactly as put"
        );
    }
    let expected: serde_json::Value = sample_values()
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect::<serde_json::Map<_, _>>()
        .into();
    assert_eq!(state_of(&store).await, expected);

    let reborn = fresh_store(&dir);
    assert_eq!(
        state_of(&reborn).await,
        expected,
        "the route wrote through to the file, not only to memory"
    );
}

#[tokio::test]
async fn a_put_replaces_the_earlier_value() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = fresh_store(&dir);
    let first = send(&store, put_request("zoom", "1.5")).await;
    assert_eq!(first.status(), StatusCode::OK, "the first put must land");
    assert_eq!(state_of(&store).await["zoom"], serde_json::json!(1.5));

    let response = send(&store, put_request("zoom", "2")).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state_of(&store).await["zoom"], serde_json::json!(2));
}

#[tokio::test]
async fn an_unknown_key_is_refused_and_the_state_stands() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = fresh_store(&dir);

    let response = send(&store, put_request("layout", "{}")).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "user_state_key");
    let message = json["error"]["message"]
        .as_str()
        .expect("the envelope includes a message");
    assert!(
        message.contains("editor_settings, zoom, recent_files, commands_history"),
        "the refusal names what is allowed: {message}"
    );
    assert_eq!(state_of(&store).await, all_null());
    assert!(
        !dir.path().join(USER_STATE_FILE).exists(),
        "a refused put creates no file"
    );
}

#[tokio::test]
async fn an_over_cap_body_is_refused_and_the_state_stands() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = fresh_store(&dir);
    let body = format!("\"{}\"", "x".repeat(STATE_BUCKET_VALUE_CAP - 1));
    assert_eq!(
        body.len(),
        STATE_BUCKET_VALUE_CAP + 1,
        "one byte past the cap"
    );

    let response = send(&store, put_request("zoom", body)).await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "user_state_too_large");
    let message = json["error"]["message"]
        .as_str()
        .expect("the envelope includes a message");
    assert!(
        message.contains(&format!("{} bytes", STATE_BUCKET_VALUE_CAP + 1))
            && message.contains(&format!("{STATE_BUCKET_VALUE_CAP} bytes")),
        "the refusal names actual and cap: {message}"
    );
    assert_eq!(state_of(&store).await, all_null());
    assert!(
        !dir.path().join(USER_STATE_FILE).exists(),
        "a refused put creates no file"
    );
}

#[tokio::test]
async fn a_body_exactly_at_the_cap_is_accepted() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = fresh_store(&dir);
    let body = format!("\"{}\"", "x".repeat(STATE_BUCKET_VALUE_CAP - 2));
    assert_eq!(body.len(), STATE_BUCKET_VALUE_CAP);

    let response = send(&store, put_request("zoom", body)).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state_of(&store).await["zoom"],
        serde_json::Value::String("x".repeat(STATE_BUCKET_VALUE_CAP - 2))
    );
}

#[tokio::test]
async fn a_non_json_body_is_refused_and_the_state_stands() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = fresh_store(&dir);

    for body in ["{", "", "not json at all"] {
        let response = send(&store, put_request("zoom", body)).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "for body {body:?}"
        );
        let json = json_body(response).await;
        assert_eq!(json["error"]["code"], "user_state_not_json", "for {body:?}");
    }
    let response = send(
        &store,
        put_request("zoom", Body::from(vec![0xff, 0xfe, b'"'])),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "bytes that are not UTF-8 are not JSON either"
    );
    assert_eq!(
        json_body(response).await["error"]["code"],
        "user_state_not_json"
    );
    assert_eq!(state_of(&store).await, all_null());
    assert!(
        !dir.path().join(USER_STATE_FILE).exists(),
        "a refused put creates no file"
    );
}

#[tokio::test]
async fn an_unknown_key_is_refused_before_the_body_is_judged() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = fresh_store(&dir);

    let response = send(&store, put_request("layout", "{")).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "user_state_key",
        "the key refusal wins over the body refusal"
    );
}

#[tokio::test]
async fn a_failed_write_answers_the_server_error_envelope_and_keeps_the_value() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // A directory in the file's place: the rename over it fails.
    std::fs::create_dir(dir.path().join(USER_STATE_FILE)).expect("directory in the file's place");
    let store = fresh_store(&dir);

    let response = send(&store, put_request("zoom", "3")).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = json_body(response).await;
    assert_eq!(json["error"]["code"], "user_state_io");
    assert_eq!(
        state_of(&store).await["zoom"],
        serde_json::json!(3),
        "the in-memory value is the source of truth; a failed persist is degradation"
    );
}
