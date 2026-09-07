//! Shared live-server helpers for STT integration tests.

#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use gateway_stt::SpeechService;
use gateway_stt_engine::test_fixtures::native::require_fixture;
use tower::ServiceExt as _;

pub(crate) fn require_model() -> PathBuf {
    require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &native_fixture_root(),
        "ggml-tiny.en.bin",
    )
}

pub(crate) fn jfk_samples() -> Vec<f32> {
    let path = require_fixture(
        "PROMPTFORGE_WHISPER_AUDIO",
        &native_fixture_root(),
        "jfk.wav",
    );
    let mut reader = hound::WavReader::open(path).expect("JFK fixture opens");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
    assert_eq!(spec.channels, 1, "fixture must be mono");
    assert_eq!(spec.bits_per_sample, 16, "fixture must be 16-bit PCM");
    reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.expect("fixture sample decodes")) / 32_768.0)
        .collect()
}

fn native_fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../gateway-stt-backend-whisper/tests/fixtures")
}

#[test]
fn native_integration_helpers_keep_the_backend_fixture_root() {
    assert_eq!(
        native_fixture_root(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../gateway-stt-backend-whisper/tests/fixtures")
    );
}

pub(crate) fn fixture_service(with_final: bool) -> SpeechService {
    let source = require_model();
    fixture_service_with_models(&source, with_final.then_some(source.as_path()))
}

pub(crate) fn fixture_service_with_models(
    interim_model: &Path,
    final_model: Option<&Path>,
) -> SpeechService {
    let interim_model = interim_model.to_path_buf();
    let final_model = final_model.map(Path::to_path_buf);
    std::thread::spawn(move || {
        fixture_service_with_models_on_dedicated_thread(&interim_model, final_model.as_deref())
    })
    .join()
    .expect("fixture service startup thread succeeds")
}

fn fixture_service_with_models_on_dedicated_thread(
    interim_model: &Path,
    final_model: Option<&Path>,
) -> SpeechService {
    let cache = tempfile::tempdir().expect("cache tempdir");
    let interim_source = interim_model.display().to_string().replace('\\', "/");
    let final_source = final_model.map(|path| path.display().to_string().replace('\\', "/"));
    let cache_path = cache.path().display().to_string().replace('\\', "/");
    let final_model = if let Some(source) = final_source {
        format!(
            "[[stt_model]]\nname = \"speech-final\"\nrole = \"final\"\nsource = {source:?}\nvram_gb = 1.0\n"
        )
    } else {
        String::new()
    };
    let profile_models = if final_model.is_empty() {
        "[\"speech\"]"
    } else {
        "[\"speech\", \"speech-final\"]"
    };
    let catalog = gateway_config::Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
         [local]\ncache_dir = {cache_path:?}\n\
         [stt]\nwindow_seconds = 8\ninterval_ms = 400\n\
         [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {interim_source:?}\nvram_gb = 1.0\n\
         {final_model}[[profile]]\nname = \"work\"\nmodels = {profile_models}\n"
    ))
    .expect("fixture catalog parses");
    let config = catalog
        .select_profile(&gateway_config::ProfileName::parse("work").expect("profile name"))
        .expect("fixture profile selects");
    let service = SpeechService::new();
    let prepared = service
        .prepare(&config, None)
        .expect("fixture artifacts prepare");
    let replacement = service
        .begin_replacement(prepared)
        .expect("fixture engine loads");
    service
        .commit_replacement(replacement)
        .expect("fixture generation publishes");
    service
}

pub(crate) fn copy_model_replacing_token(
    source: &Path,
    destination_dir: &Path,
    from: &[u8],
    to: &[u8],
) -> PathBuf {
    assert_eq!(
        from.len(),
        to.len(),
        "model token replacement preserves size"
    );
    let mut model = std::fs::read(source).expect("source model reads");
    let mut replacements = 0usize;
    for offset in 0..=model.len().saturating_sub(from.len()) {
        if model[offset..].starts_with(from) {
            model[offset..offset + from.len()].copy_from_slice(to);
            replacements += 1;
        }
    }
    assert!(
        replacements > 0,
        "source model vocabulary contains {:?}",
        String::from_utf8_lossy(from)
    );
    let destination = destination_dir.join("distinct-final-model.bin");
    std::fs::write(&destination, model).expect("distinct final model writes");
    destination
}

fn wav_f32(samples: &[f32]) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
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
        .expect("WAV writer builds");
        for sample in samples {
            writer.write_sample(*sample).expect("WAV sample writes");
        }
        writer.finalize().expect("WAV finalizes");
    }
    bytes.into_inner()
}

fn multipart_body(file: &[u8], model: &str) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "gateway-stt-integration-boundary";
    let mut body = format!(
        "--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"model\"\r\n\r\n\
         {model}\r\n\
         --{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"response_format\"\r\n\r\n\
         json\r\n\
         --{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
         Content-Type: audio/wav\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (BOUNDARY.to_owned(), body)
}

pub(crate) async fn transcribe_batch(
    service: SpeechService,
    model: &str,
    samples: &[f32],
) -> (StatusCode, serde_json::Value) {
    let (boundary, body) = multipart_body(&wav_f32(samples), model);
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
                .expect("batch request builds"),
        )
        .await
        .expect("batch route answers");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("batch response body reads");
    let json = serde_json::from_slice(&body).expect("batch response is JSON");
    (status, json)
}
