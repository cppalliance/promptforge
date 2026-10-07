//! Shared native fixture helpers for the backend's flat test targets.

#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::path::{Path, PathBuf};

use gateway_stt_engine::test_fixtures::native::require_fixture;

pub(crate) fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub(crate) fn jfk_samples() -> Vec<f32> {
    let path = require_fixture("PROMPTFORGE_WHISPER_AUDIO", &fixture_dir(), "jfk.wav");
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
