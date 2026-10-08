//! Tests for the batch transcription endpoint and its response formats.

use super::*;
use crate::test_fixtures::{SCRIPTED_SILERO_MODEL, ScriptedSilero};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedDetector};
use std::path::PathBuf;
use tower::ServiceExt;

fn wav(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .expect("writer builds");
        for sample in samples {
            writer.write_sample(*sample).expect("sample writes");
        }
        writer.finalize().expect("WAV finalizes");
    }
    bytes.into_inner()
}

#[test]
fn wav_decode_accepts_the_stt_wire_sample_rate() {
    let (samples, duration) = decode_wav(&wav(&[0, i16::MAX])).expect("WAV decodes");
    assert_eq!(samples.len(), 2);
    assert!(samples[1] > 0.99);
    assert!((duration - 2.0 / 16_000.0).abs() < f64::EPSILON);
}

#[test]
fn verbose_json_honors_segment_granularity() {
    let response = response(
        &TranscriptionForm {
            file: Vec::new(),
            model: "speech".to_owned(),
            prompt: String::new(),
            format: ResponseFormat::VerboseJson,
            granularities: vec![TimestampGranularity::Segment],
        },
        "hello".to_owned(),
        1.25,
    );
    let json = serde_json::to_value(response).expect("response serializes");
    assert_eq!(json["language"], "en");
    assert_eq!(json["text"], "hello");
    assert_eq!(json["duration"], 1.25);
    assert_eq!(json["segments"][0]["end"], 1.25);
}

#[test]
fn verbose_json_defaults_to_segment_timestamps() {
    let granularities = default_granularities();
    let response = response(
        &TranscriptionForm {
            file: Vec::new(),
            model: "speech".to_owned(),
            prompt: String::new(),
            format: ResponseFormat::VerboseJson,
            granularities,
        },
        "hello".to_owned(),
        1.25,
    );
    let json = serde_json::to_value(response).expect("response serializes");
    assert_eq!(json["segments"][0]["text"], "hello");
}

#[test]
fn compact_json_contains_only_text() {
    let response = response(
        &TranscriptionForm {
            file: Vec::new(),
            model: "speech".to_owned(),
            prompt: String::new(),
            format: ResponseFormat::Json,
            granularities: Vec::new(),
        },
        "hello".to_owned(),
        1.0,
    );
    assert_eq!(
        serde_json::to_value(response).expect("response serializes"),
        serde_json::json!({"text": "hello"})
    );
}

fn multipart_body(file: &[u8], fields: &[(&str, &str)]) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "gateway-stt-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
          Content-Type: audio/wav\r\n\r\n",
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (BOUNDARY.to_owned(), body)
}

#[tokio::test]
async fn an_unloaded_model_is_not_found() {
    let (boundary, body) = multipart_body(
        &wav(&vec![0; 16_000]),
        &[("model", "not-loaded"), ("response_format", "json")],
    );
    let response = routes(GenerationState::default())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("route answers");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

async fn post(fields: &[(&str, &str)]) -> (StatusCode, serde_json::Value) {
    post_to(&GenerationState::default(), fields).await
}

async fn post_to(
    state: &GenerationState,
    fields: &[(&str, &str)],
) -> (StatusCode, serde_json::Value) {
    let (boundary, body) = multipart_body(&wav(&vec![0; 16_000]), fields);
    let response = routes(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("route answers");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    (status, serde_json::from_slice(&body).expect("body is JSON"))
}

#[tokio::test]
async fn a_language_other_than_en_is_rejected_before_model_selection() {
    let (status, json) = post(&[("model", "not-loaded"), ("language", "fr")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["code"], "malformed_request");
    assert_eq!(
        json["error"]["message"],
        "malformed request: unsupported transcription language fr; only en is transcribed"
    );
}

#[tokio::test]
async fn the_en_language_passes_validation() {
    let (status, json) = post(&[("model", "not-loaded"), ("language", "en")]).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "en reaches model selection: {json}"
    );
}

#[tokio::test]
async fn an_audio_file_over_25_mib_is_rejected_before_decode() {
    let oversized = vec![0_u8; MAX_AUDIO_BYTES + 1];
    let (boundary, body) = multipart_body(&oversized, &[("model", "speech")]);
    let response = routes(GenerationState::default())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/audio/transcriptions")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("route answers");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
    assert_eq!(
        json["error"]["message"],
        "audio file exceeds the 25 MiB limit"
    );
}

/// A generation whose runtime holds `[stt] vocabulary = ["WG21"]`, and the
/// decoder that records the requests it serves.
fn guided_generation() -> (GenerationState, ScriptedDecoder) {
    let decoder = ScriptedDecoder::new();
    let state = GenerationState::default();
    state
        .publish_scripted_guided(
            decoder.clone(),
            PathBuf::from(SCRIPTED_SILERO_MODEL),
            ScriptedSilero::new(ScriptedDetector::new([])),
            vec!["WG21".to_owned()],
        )
        .expect("the scripted runtime loads");
    (state, decoder)
}

#[tokio::test]
async fn prompt_terms_follow_the_configured_vocabulary_into_the_decode_request() {
    let (state, decoder) = guided_generation();

    let (status, json) = post_to(
        &state,
        &[("model", "scripted-interim"), ("prompt", " MCP, , GGUF ")],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{json}");
    let requests = decoder.requests();
    assert_eq!(requests.len(), 1, "one decode serves the request");
    assert_eq!(
        requests[0].guidance(),
        ["WG21", "MCP", "GGUF"],
        "the configured terms lead the client's prompt terms"
    );
    state.shutdown();
}

#[tokio::test]
async fn a_request_without_a_prompt_decodes_with_the_configured_vocabulary_alone() {
    let (state, decoder) = guided_generation();

    let (status, json) = post_to(&state, &[("model", "scripted-interim")]).await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(decoder.requests()[0].guidance(), ["WG21"]);
    state.shutdown();
}
