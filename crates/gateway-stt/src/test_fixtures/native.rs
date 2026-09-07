//! Native fixture loading for ignored crate tests.

#![expect(
    clippy::expect_used,
    reason = "test fixture loading fails immediately when required native assets are invalid"
)]

use std::path::{Path, PathBuf};

use gateway_stt_engine::test_fixtures::native::require_fixture;

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
fn native_service_fixtures_keep_the_backend_fixture_root() {
    assert_eq!(
        native_fixture_root(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../gateway-stt-backend-whisper/tests/fixtures")
    );
}
