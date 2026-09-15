use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
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
        TranscriptionForm {
            file: Vec::new(),
            model: "speech".to_owned(),
            language: Some("en".to_owned()),
            format: ResponseFormat::VerboseJson,
            granularities: vec![TimestampGranularity::Segment],
        },
        "hello".to_owned(),
        1.25,
    );
    let json = serde_json::to_value(response).expect("response serializes");
    assert_eq!(json["text"], "hello");
    assert_eq!(json["duration"], 1.25);
    assert_eq!(json["segments"][0]["end"], 1.25);
}

#[test]
fn verbose_json_defaults_to_segment_timestamps() {
    let granularities = default_granularities();
    let response = response(
        TranscriptionForm {
            file: Vec::new(),
            model: "speech".to_owned(),
            language: None,
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
        TranscriptionForm {
            file: Vec::new(),
            model: "speech".to_owned(),
            language: None,
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
