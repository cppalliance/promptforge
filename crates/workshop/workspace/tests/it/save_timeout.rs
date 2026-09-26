//! The save-timeout behavior on the workspace routes: a
//! `PUT /workspace/file` whose write outlasts the route deadline answers
//! a 408 whose body is the JSON error envelope, and the blocking write -
//! abandoned, not cancelled - still lands on disk once the test releases
//! it.

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use tower::ServiceExt;

use workshop_workspace::Workspace;

/// The test-only route deadline: short enough that the stalled write's
/// 408 is reachable without waiting out the production 10 seconds.
const TEST_DEADLINE: Duration = Duration::from_secs(1);

#[tokio::test]
async fn a_write_that_outlasts_its_deadline_answers_408_and_still_lands() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = dir.path().join("note.txt");
    let workspace = Workspace::new();
    workspace.grant(dir.path()).expect("grant the tempdir");

    // Arm the stall: the write blocks on the blocking pool until released,
    // so the route deadline elapses first and its 408 is observable.
    let stall = workspace.stall_next_write_for_test();

    let router = workshop_workspace::routes_with_deadline(workspace, TEST_DEADLINE);
    let request = Request::builder()
        .method("PUT")
        .uri("/workspace/file")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "path": file, "text": "late write" }).to_string(),
        ))
        .expect("static request parts are valid");

    let response = router
        .oneshot(request)
        .await
        .expect("the router is infallible");

    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .map(axum::http::header::HeaderValue::as_bytes),
        Some(b"application/json".as_slice()),
        "the deadline answers JSON"
    );

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already");
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).expect("the body is JSON");
    let expected = serde_json::to_value(workshop_protocol::ErrorEnvelope::new(
        workshop_support::deadline_elapsed_message(TEST_DEADLINE),
        workshop_support::DEADLINE_ELAPSED_CODE,
    ))
    .expect("the envelope serializes");
    assert_eq!(envelope, expected, "the 408 body is the wire envelope");

    // The write was abandoned by the deadline, not cancelled: releasing the
    // stall lets it land on disk.
    stall.release();
    stall.await_completion();
    assert_eq!(
        std::fs::read_to_string(&file).expect("the file exists"),
        "late write",
        "the released write still lands on disk"
    );
}
