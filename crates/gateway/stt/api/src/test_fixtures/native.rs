//! Native fixture loading: re-exports the engine's `require_fixture` and
//! names the fixture whisper build and final model for downstream
//! integration tests, and adds this crate's model and audio loaders for its
//! own ignored tests.

use std::path::{Path, PathBuf};

pub use gateway_stt_engine::test_fixtures::native::require_fixture;

/// Returns the `[stt] whisper_backend` spelling native fixture configs write:
/// `PROMPTFORGE_WHISPER_BACKEND` when set, otherwise `cpu`. The spelling is
/// passed through unchecked, so an unknown value fails the fixture's config
/// parse with the error that names the accepted values.
#[must_use]
pub fn fixture_whisper_backend() -> String {
    std::env::var_os("PROMPTFORGE_WHISPER_BACKEND").map_or_else(
        || "cpu".to_owned(),
        |value| value.to_string_lossy().into_owned(),
    )
}

/// Returns the final-role model native fixtures load beside `interim`: the
/// file `PROMPTFORGE_WHISPER_FINAL_MODEL` names when set, otherwise
/// `interim` itself.
///
/// # Panics
///
/// Panics with the named path when the variable is set but names no file.
#[must_use]
pub fn fixture_final_model(interim: &Path) -> PathBuf {
    let Some(path) = std::env::var_os("PROMPTFORGE_WHISPER_FINAL_MODEL").map(PathBuf::from) else {
        return interim.to_path_buf();
    };
    assert!(
        path.is_file(),
        "PROMPTFORGE_WHISPER_FINAL_MODEL names a model file, but {} is not one",
        path.display()
    );
    path
}

#[cfg(all(test, not(miri)))]
pub(crate) fn require_model() -> PathBuf {
    require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &native_fixture_root(),
        "ggml-tiny.en.bin",
    )
}

#[cfg(all(test, not(miri)))]
#[expect(
    clippy::expect_used,
    reason = "test fixture loading fails immediately when required native assets are invalid"
)]
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

#[cfg(all(test, not(miri)))]
fn native_fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../backend-whisper/tests/fixtures")
}

#[cfg(all(test, not(miri)))]
#[test]
fn native_service_fixtures_keep_the_backend_fixture_root() {
    assert_eq!(
        native_fixture_root(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../backend-whisper/tests/fixtures")
    );
}
