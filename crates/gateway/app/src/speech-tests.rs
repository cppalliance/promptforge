//! The speech and transcription routes' auth ordering and error envelopes, through the real router.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::workshop_state;

#[cfg(feature = "stt")]
#[test]
fn speech_snapshot_serializes_only_generic_facade_facts() {
    let snapshot = super::SpeechSnapshot::from(gateway_stt::SpeechService::new().status());

    assert_eq!(
        serde_json::json!(snapshot),
        serde_json::json!({
            "configured": false,
            "ready": false,
            "gpu": false,
        })
    );
}

#[cfg(feature = "stt")]
#[tokio::test]
async fn transcription_checks_bearer_auth_before_multipart_extraction() {
    let response = build_router(workshop_state(), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("content-type", "not-multipart")
                .body(Body::from("not multipart"))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "auth refuses the request before its malformed body is extracted"
    );
}

#[cfg(feature = "stt")]
#[tokio::test]
async fn authenticated_multipart_rejection_uses_the_openai_error_envelope() {
    let response = build_router(workshop_state(), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("authorization", "Bearer test-token")
                .header("content-type", "not-multipart")
                .body(Body::from("not multipart"))
                .expect("request builds"),
        )
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
    assert_eq!(
        json,
        serde_json::json!({
            "error": {
                "message": "malformed request: Invalid `boundary` for `multipart/form-data` request",
                "type": "invalid_request_error",
                "code": "malformed_request",
            }
        })
    );
}

#[cfg(feature = "stt")]
#[tokio::test]
async fn batch_validation_preserves_the_gateway_error_message_contract() {
    let body = "--empty\r\n\
                Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
                Content-Type: audio/wav\r\n\r\n\
                bytes\r\n\
                --empty--\r\n";
    let response = build_router(workshop_state(), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("authorization", "Bearer test-token")
                .header("content-type", "multipart/form-data; boundary=empty")
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
    assert_eq!(
        json,
        serde_json::json!({
            "error": {
                "message": "malformed request: missing multipart field model",
                "type": "invalid_request_error",
                "code": "malformed_request",
            }
        })
    );
}

#[cfg(feature = "stt")]
#[tokio::test]
async fn unloaded_transcription_model_returns_openai_model_not_found() {
    const BOUNDARY: &str = "gateway-stt-boundary";
    let mut wav = vec![
        b'R', b'I', b'F', b'F', 36, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ', 16, 0,
        0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0, b'd', b'a', b't', b'a',
        0, 0, 0, 0,
    ];
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nghost\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.append(&mut wav);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    let response = build_router(workshop_state(), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("authorization", "Bearer test-token")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
    assert_eq!(json["error"]["code"], "model_not_found");
}

// The speech route's auth ordering through the real router. The route
// is unconditional, so these tests sit beside, not inside, the
// stt-gated `transcription_auth_tests` module.

#[tokio::test]
async fn speech_checks_bearer_auth_before_json_extraction() {
    let response = build_router(workshop_state(), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/speech")
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "auth refuses the request before its malformed body is extracted"
    );
}

#[tokio::test]
async fn authenticated_malformed_json_uses_the_openai_error_envelope() {
    let response = build_router(workshop_state(), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/speech")
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
