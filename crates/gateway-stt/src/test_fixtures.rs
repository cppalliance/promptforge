//! Native fixtures used only by this crate's unit tests.

#[cfg(test)]
use std::path::{Path, PathBuf};

#[cfg(feature = "test-fixtures")]
pub use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};

#[cfg(feature = "test-fixtures")]
use crate::SttRuntime;
#[cfg(feature = "test-fixtures")]
use gateway_stt_engine::{SttEngine, TranscribeError};

/// Builds a speech runtime around deterministic scripted workers.
///
/// # Errors
/// Returns engine policy, startup, or worker construction failures.
#[cfg(feature = "test-fixtures")]
pub fn scripted_runtime(
    factory: ScriptedModelFactory,
    window_seconds: u64,
    interval_ms: u64,
) -> Result<SttRuntime, TranscribeError> {
    let engine = SttEngine::new(factory, window_seconds, interval_ms)?;
    let final_name = engine.has_final_pass().then(|| "scripted-final".to_owned());
    Ok(SttRuntime::from_scripted_engine(
        engine,
        "scripted-interim".to_owned(),
        final_name,
        Vec::new(),
    ))
}

#[cfg(test)]
pub(crate) fn require_model() -> PathBuf {
    require_fixture("PROMPTFORGE_WHISPER_MODEL", "ggml-tiny.en.bin")
}

#[cfg(test)]
pub(crate) fn jfk_samples() -> Vec<f32> {
    let path = require_fixture("PROMPTFORGE_WHISPER_AUDIO", "jfk.wav");
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

#[cfg(test)]
fn require_fixture(variable: &str, fallback: &str) -> PathBuf {
    let path = std::env::var_os(variable).map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../gateway-stt-backend-whisper/tests/fixtures")
                .join(fallback)
        },
        PathBuf::from,
    );
    assert!(
        path.is_file(),
        "native test fixture is missing: {}",
        path.display()
    );
    path
}
