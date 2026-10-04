//! Speech request admission: auth before parsing, model lookup, the kind
//! guard, voice validation, and dominion queue admission.

use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;

use super::{
    bytes_within, gated_audio_backend, recording_speech_backend, spawn_speech, speech_body,
    speech_gateway,
};
use crate::support::{
    PHASE_TIMEOUT, TestServer, join_within, json_within, next_arrival, send_within,
};

/// A model configured for a non-speech kind is rejected on the speech route
/// with 400 and `kind_mismatch` before any queue admission or upstream call.
#[tokio::test]
async fn non_speech_kinds_are_rejected_on_the_speech_route() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let toml = format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "chat-model"
description = "a chat model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "embed-model"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "reranker"
kind = "classifier"
description = "a classifier model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    for model in ["chat-model", "embed-model", "reranker"] {
        let response = send_within(
            reqwest::Client::new()
                .post(format!("http://{}/v1/audio/speech", gateway.addr))
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": model,
                    "input": "say something",
                    "voice": "alloy"
                })),
        )
        .await;
        assert_eq!(response.status().as_u16(), 400, "model {model}");
        let body = json_within(response).await;
        assert_eq!(
            body.pointer("/error/code").and_then(Value::as_str),
            Some("kind_mismatch"),
            "model {model}"
        );
    }
    assert!(
        recorder.lock().unwrap().is_empty(),
        "a kind-mismatched request must never reach the backend"
    );
    gateway.shutdown().await;
}

/// A voice outside the model's catalog list is rejected with 400
/// `invalid_voice` naming the valid voices, before any upstream call.
#[tokio::test]
async fn unknown_voice_is_rejected_naming_the_valid_voices() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "coral"
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 400);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("invalid_voice")
    );
    assert_eq!(
        body.pointer("/error/type").and_then(Value::as_str),
        Some("invalid_request_error")
    );
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap();
    assert!(
        message.contains("alloy") && message.contains("nova"),
        "the error names the valid voices: {message}"
    );
    assert!(
        recorder.lock().unwrap().is_empty(),
        "a voice rejection never reaches the backend"
    );
    gateway.shutdown().await;
}

/// The voice check runs before dominion queue admission: with the only
/// concurrency slot held and the pool on the fail-fast `reject` policy, a
/// bad voice still receives 400 rather than the pool's 429, while a valid
/// voice receives the 429 - proving the pool really was full.
#[tokio::test]
async fn voice_validation_precedes_queue_admission() {
    let (backend, mut arrivals) = gated_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), Some((1, 100, "reject"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    let bad_voice = send_within(client.post(&url).bearer_auth("test-token").json(
        &serde_json::json!({
            "model": "tts-model",
            "input": "say something",
            "voice": "coral"
        }),
    ))
    .await;
    assert_eq!(
        bad_voice.status().as_u16(),
        400,
        "voice validation fires before admission, so a full pool cannot turn it into a 429"
    );
    let body = json_within(bad_voice).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("invalid_voice")
    );

    let good_voice = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(
        good_voice.status().as_u16(),
        429,
        "the pool really was full: a valid request is rejected at admission"
    );
    let body = json_within(good_voice).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("queue_rejected")
    );

    release_first.send(()).unwrap();
    let first = join_within(first).await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    assert_eq!(bytes_within(first).await, b"audio-chunk-1;audio-chunk-2;");
    gateway.shutdown().await;
}

/// A model with an empty catalog `voices` list skips the voice check:
/// any voice name passes and is forwarded verbatim.
#[tokio::test]
async fn an_empty_voices_list_skips_the_voice_check() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&[]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "anything-goes"
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    assert_eq!(
        seen[0].body.get("voice").and_then(Value::as_str),
        Some("anything-goes"),
        "the unchecked voice is forwarded verbatim"
    );
    gateway.shutdown().await;
}

/// The OpenAI object voice form (`{"id": ...}`) is validated by its `id`
/// against the catalog list and forwarded verbatim.
#[tokio::test]
async fn voice_object_form_is_validated_by_id_and_forwarded() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": { "id": "nova" }
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    assert_eq!(
        seen[0].body.get("voice"),
        Some(&serde_json::json!({ "id": "nova" })),
        "the object form is forwarded unchanged"
    );
    gateway.shutdown().await;
}

/// The speech handler admits through the model's dominion queue the same
/// way chat does: with one in-flight slot and one waiting slot, the third
/// request is 503 `queue_full`.
#[tokio::test]
async fn queue_full_returns_503_when_waiting_slots_exhausted() {
    let (backend, mut arrivals) = gated_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 1, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // Exactly one of these acquires the single waiting slot; the other is 503.
    // The race is bounded by PHASE_TIMEOUT (IT-003): if admission broke so
    // neither request completes, the test fails instead of hanging.
    let mut second = spawn_speech(&client, &url);
    let mut third = spawn_speech(&client, &url);
    let (rejected, survivor) = tokio::time::timeout(PHASE_TIMEOUT, async {
        tokio::select! {
            r = &mut second => (r, third),
            r = &mut third => (r, second),
        }
    })
    .await
    .expect("neither queued request completed within the phase timeout");
    let rejected = rejected.unwrap().unwrap();
    assert_eq!(rejected.status().as_u16(), 503);
    let body = json_within(rejected).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("queue_full")
    );

    release_first.send(()).unwrap();
    let first = join_within(first).await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    let _ = bytes_within(first).await;

    let release_survivor = next_arrival(&mut arrivals).await;
    release_survivor.send(()).unwrap();
    let survivor = join_within(survivor).await.unwrap();
    assert_eq!(survivor.status().as_u16(), 200);
    let _ = bytes_within(survivor).await;
    gateway.shutdown().await;
}

/// Auth runs before the body is parsed: an unauthenticated request with a
/// malformed JSON body is refused 401, and nothing reaches the backend.
#[tokio::test]
async fn unauthenticated_speech_is_refused_before_the_body_is_parsed() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body("{not json"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 401);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("unauthorized")
    );
    assert!(
        recorder.lock().unwrap().is_empty(),
        "an unauthenticated request never reaches the backend"
    );
    gateway.shutdown().await;
}

/// An unknown model on the speech route is a 404 `model_not_found`
/// envelope, as on the chat route, and never reaches the backend.
#[tokio::test]
async fn unknown_model_on_the_speech_route_returns_model_not_found() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "ghost",
                "input": "say something",
                "voice": "alloy"
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 404);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("model_not_found")
    );
    assert!(
        recorder.lock().unwrap().is_empty(),
        "an unknown model never reaches the backend"
    );
    gateway.shutdown().await;
}
