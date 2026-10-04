//! Remote passthrough: opaque audio bytes, the forwarded `stream_format`, and
//! the response media type with its format and framing fallbacks.

use serde_json::Value;

use super::{CANNED_AUDIO, bytes_within, recording_speech_backend, speech_body, speech_gateway};
use crate::support::send_within;

/// IT-005/006 for the speech route: the backend records the request, so we
/// assert exactly what the gateway forwarded - method, path, the rewritten
/// upstream model, the intact input and voice, and the structural mp3 pin
/// reaching the provider when the client omits `response_format` - and that
/// the client's bearer is not leaked upstream. The response body is the
/// upstream's bytes, unchanged, under the upstream's own `Content-Type`.
#[tokio::test]
async fn remote_passthrough_streams_audio_bytes_unchanged() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("audio/mpeg"),
        "the upstream content type is forwarded"
    );
    assert!(
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .is_none(),
        "a streamed audio body omits Content-Length"
    );
    let body = bytes_within(response).await;
    assert_eq!(body, CANNED_AUDIO, "audio bytes pass through unchanged");

    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/audio/speech");
    assert_eq!(
        request.body.get("model").and_then(Value::as_str),
        Some("backend-tts"),
        "public model name rewritten to the upstream alias"
    );
    assert_eq!(
        request.body.get("input").and_then(Value::as_str),
        Some("hello <laugh> from the gateway"),
        "input forwarded intact, emotion tag and all: angle-bracket markup is never sanitized"
    );
    assert_eq!(
        request.body.get("voice").and_then(Value::as_str),
        Some("alloy"),
        "voice forwarded intact"
    );
    assert_eq!(
        request.body.get("response_format").and_then(Value::as_str),
        Some("mp3"),
        "the structural mp3 pin reaches the provider when the field is omitted"
    );
    assert_ne!(
        request.authorization.as_deref(),
        Some("Bearer test-token"),
        "caller bearer must not leak to the upstream"
    );
    gateway.shutdown().await;
}

/// When the upstream omits `Content-Type`, the route falls back to the
/// requested format's MIME type (the OpenAI spellings).
#[tokio::test]
async fn content_type_falls_back_to_the_format_mime_mapping() {
    let (backend, _recorder) = recording_speech_backend(None).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;
    let client = reqwest::Client::new();
    for (format, mime) in [
        ("mp3", "audio/mpeg"),
        ("opus", "audio/ogg"),
        ("aac", "audio/aac"),
        ("flac", "audio/flac"),
        ("wav", "audio/wav"),
        ("pcm", "audio/pcm"),
    ] {
        let response = send_within(
            client
                .post(format!("http://{}/v1/audio/speech", gateway.addr))
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": "tts-model",
                    "input": "format check",
                    "voice": "alloy",
                    "response_format": format,
                })),
        )
        .await;
        assert_eq!(response.status().as_u16(), 200, "format {format}");
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some(mime),
            "format {format} falls back to its MIME type"
        );
    }
    gateway.shutdown().await;
}

/// `stream_format` is forwarded verbatim: the provider sees the framing
/// selector exactly as the client sent it.
#[tokio::test]
async fn stream_format_sse_is_forwarded_to_the_upstream_body() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "alloy",
                "stream_format": "sse",
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let _ = bytes_within(response).await;
    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    assert_eq!(
        seen[0].body.get("stream_format").and_then(Value::as_str),
        Some("sse"),
        "the framing selector is forwarded into the outbound upstream body"
    );
    gateway.shutdown().await;
}

/// With `stream_format = "sse"` the fallback media type follows the framing
/// selector: an upstream that omits `Content-Type` is labeled
/// `text/event-stream`, never an audio type.
#[tokio::test]
async fn sse_framing_falls_back_to_event_stream_when_content_type_is_missing() {
    let (backend, _recorder) = recording_speech_backend(None).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "alloy",
                "stream_format": "sse",
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
        "the fallback takes the framing selector, never an audio type"
    );
    gateway.shutdown().await;
}

/// A present upstream `Content-Type` is forwarded verbatim even with
/// `stream_format = "sse"`: the framing selector drives only the fallback.
#[tokio::test]
async fn sse_framing_still_forwards_a_present_upstream_content_type() {
    let (backend, _recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "alloy",
                "stream_format": "sse",
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("audio/mpeg"),
        "a present upstream content type wins over the framing fallback"
    );
    gateway.shutdown().await;
}
