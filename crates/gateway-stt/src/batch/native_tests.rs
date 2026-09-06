//! Native batch route coverage.

#![expect(
    clippy::expect_used,
    reason = "native route fixtures fail with the invariant named"
)]

use std::io::Cursor;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

mod native_runtime {
    #[rustfmt::skip]
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/common/native_runtime.rs"));
}

fn wav_f32(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
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

fn multipart_body(file: &[u8]) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "gateway-stt-boundary";
    let mut body = format!(
        "--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"model\"\r\n\r\n\
         speech\r\n\
         --{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"response_format\"\r\n\r\n\
         verbose_json\r\n\
         --{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"timestamp_granularities[]\"\r\n\r\n\
         segment\r\n\
         --{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (BOUNDARY.to_owned(), body)
}

#[tokio::test]
#[ignore = "requires whisper test fixtures (tests/fixtures/)"]
async fn verbose_round_trip_accepts_literal_timestamp_granularities_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = crate::test_fixtures::require_model()
        .display()
        .to_string()
        .replace('\\', "/");
    let cache = dir.path().display().to_string().replace('\\', "/");
    let catalog = gateway_config::Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
         [local]\ncache_dir = {cache:?}\n\
         [workshop]\n\
         [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {source:?}\n\
         vram_gb = 1.0\n\
         [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n"
    ))
    .expect("catalog parses");
    let config = catalog
        .select_profile(&gateway_config::ProfileName::parse("work").expect("name"))
        .expect("profile selects");
    let service = native_runtime::start(config);
    let (boundary, body) = multipart_body(&wav_f32(&crate::test_fixtures::jfk_samples()));
    let response = service
        .routes()
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
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
    assert!(
        json["text"]
            .as_str()
            .is_some_and(|text| text.to_lowercase().contains("country"))
    );
    assert_eq!(json["segments"][0]["start"], 0.0);
    native_runtime::shutdown(service);
}
