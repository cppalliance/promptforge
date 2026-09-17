//! The relay routes' request-boundary behavior: the envelope on a
//! malformed body, and auth before any body parse.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::workshop_state;

#[tokio::test]
async fn malformed_json_answers_with_the_openai_error_envelope() {
    let response = build_router(workshop_state(), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("authorization", "Bearer test-token")
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
    assert_eq!(json["error"]["code"], "malformed_request");
    assert_eq!(json["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn auth_runs_before_the_body_is_parsed() {
    // No credential and no recorded peer: refused during parts extraction,
    // before the malformed body is read.
    let response = build_router(workshop_state(), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
