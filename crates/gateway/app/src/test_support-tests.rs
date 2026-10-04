//! Tests for the scripted STT fixtures: injection, the batch error contract, and publication through gateway surfaces.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};
use tower::ServiceExt;

use super::*;

fn transcription_body() -> (String, Vec<u8>) {
    const BOUNDARY: &str = "scripted-stt-boundary";
    let mut wav = vec![
        b'R', b'I', b'F', b'F', 38, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ', 16, 0,
        0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0, b'd', b'a', b't', b'a',
        2, 0, 0, 0, 0, 32,
    ];
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n\
         scripted-interim\r\n\
         --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"sample.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.append(&mut wav);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (BOUNDARY.to_owned(), body)
}

#[tokio::test]
async fn scripted_workers_can_be_injected_without_a_production_constructor() {
    const TRANSCRIPT: &str = "gateway scripted route sentinel";

    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
    )
    .expect("config parses");
    let decoder = ScriptedDecoder::new();
    decoder.push_text(TRANSCRIPT);
    let state = app_state_with_scripted_stt(config, ScriptedModelFactory::new(decoder.clone()))
        .expect("scripted state builds");
    let (boundary, body) = transcription_body();

    let response = build_router(state, None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("authorization", "Bearer test-token")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    let response: serde_json::Value = serde_json::from_slice(&body).expect("response body is JSON");
    assert_eq!(response["text"], TRANSCRIPT);
    let requests = decoder.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].mode(),
        gateway_stt::test_fixtures::DecodeMode::Interim
    );
    assert_eq!(requests[0].samples(), &[0.25]);
    assert!(requests[0].guidance().is_empty());
    assert!(requests[0].finalized().is_empty());
}

#[tokio::test]
async fn batch_inference_preserves_the_gateway_error_message_contract() {
    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
    )
    .expect("config parses");
    let decoder = ScriptedDecoder::new();
    decoder.push_error("scripted inference sentinel");
    let state = app_state_with_scripted_stt(config, ScriptedModelFactory::new(decoder))
        .expect("scripted state builds");
    let (boundary, body) = transcription_body();

    let response = build_router(state, None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header("authorization", "Bearer test-token")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    let response: serde_json::Value = serde_json::from_slice(&body).expect("response body is JSON");
    assert_eq!(
        response,
        serde_json::json!({
            "error": {
                "message": "transcription failed",
                "type": "server_error",
                "code": "transcription_error",
            }
        })
    );
}

async fn get_json(state: AppState, uri: &'static str) -> serde_json::Value {
    let response = build_router(state, None)
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    serde_json::from_slice(&body).expect("response body is JSON")
}

#[tokio::test]
async fn ready_scripted_pair_is_published_through_gateway_surfaces() {
    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
    )
    .expect("config parses");
    let factory = ScriptedModelFactory::new(ScriptedDecoder::new())
        .with_final(ScriptedDecoder::new())
        .with_gpu_available(true);
    let state = app_state_with_scripted_stt(config, factory).expect("scripted state builds");
    let service = state.speech.clone();

    let status = get_json(state.clone(), "/admin/status").await;
    assert_eq!(
        status["speech"],
        serde_json::json!({
            "configured": true,
            "ready": true,
            "gpu": true,
        })
    );
    let speech_endpoint = status["endpoints"]
        .as_array()
        .expect("endpoints are an array")
        .iter()
        .find(|entry| entry["path"] == "/v1/audio/transcriptions")
        .expect("speech endpoint is present");
    assert_eq!(speech_endpoint["ready"], true);
    assert_eq!(speech_endpoint["provisioning"], false);

    let catalog = get_json(state, "/v1/models").await;
    assert_eq!(
        catalog["data"]
            .as_array()
            .expect("catalog data")
            .iter()
            .map(|model| model["id"].as_str().expect("model id"))
            .collect::<Vec<_>>(),
        ["scripted-interim", "scripted-final", "realtime-transcribe"]
    );
    service.shutdown();
}
