//! The workspace save deadline through the full server router: a
//! `PUT /workspace/file` whose write outlasts the default deadline
//! answers a 408 whose body is the JSON error envelope. The workspace
//! crate pins the abandoned write landing on a short test deadline; this
//! test pins that the server mounts the routes under the deadline tier.

use reqwest::StatusCode;
use workshop_support::{DEADLINE_ELAPSED_CODE, DEFAULT_DEADLINE, deadline_elapsed_message};
use workshop_workspace::Workspace;

use crate::common::{spawn_router, test_config};

#[tokio::test]
async fn a_stalled_save_answers_the_json_408_through_the_full_router() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let (state, base) = spawn_router(&test_config("http://127.0.0.1:1", state_dir.path())).await;
    let workspace = state
        .registry()
        .require::<Workspace>()
        .expect("the workspace is registered");
    let files = tempfile::TempDir::new().expect("tempdir");
    let file = files.path().join("note.txt");
    workspace.grant(files.path()).expect("grant the tempdir");
    // The write blocks on the blocking pool until released, so the
    // production deadline elapses first and its 408 is observable.
    let stall = workspace.stall_next_write_for_test();

    let url = format!("{}/workspace/file", base.replacen("ws://", "http://", 1));
    let response = reqwest::Client::new()
        .put(url)
        .header("content-type", "application/json")
        .body(serde_json::json!({ "path": file, "text": "late write" }).to_string())
        .send()
        .await
        .expect("the route answers");

    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .map(reqwest::header::HeaderValue::as_bytes),
        Some(b"application/json".as_slice()),
        "the deadline answers JSON"
    );
    let envelope: serde_json::Value = response.json().await.expect("the body is JSON");
    let expected = serde_json::to_value(workshop_protocol::ErrorEnvelope::new(
        deadline_elapsed_message(DEFAULT_DEADLINE),
        DEADLINE_ELAPSED_CODE,
    ))
    .expect("the envelope serializes");
    assert_eq!(envelope, expected, "the 408 body is the wire envelope");

    // Let the abandoned write land before the tempdir goes.
    stall.release();
    stall.await_completion();
}
