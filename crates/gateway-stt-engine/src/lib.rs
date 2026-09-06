//! Whisper transcription on dedicated worker threads.
//!
//! [`SttEngine`] owns two worker threads: the interim worker holds the
//! streaming model and transcribes sliding windows, and the final-pass
//! worker (`FinalTranscriber`, present when [`EngineConfig::final_model`] is
//! set) holds the larger model and transcribes completed speech segments in
//! the background while the user is still talking. Callers hand owned sample
//! buffers through channels and await transcripts on oneshots, so the
//! blocking CPU-bound inference never touches the tokio executor. The pure
//! helpers ([`is_silence`], [`tail`]) are the session's silence gate:
//! whisper hallucinates plausible text on silent input, so quiet windows are
//! never sent to the model.

mod engine;
mod error;
mod final_pass;
mod prompt;
mod segment;
mod slot;
mod worker;

pub use engine::{EngineConfig, SttEngine};
pub use error::TranscribeError;
pub use segment::Segmenter;
pub use slot::SttSlot;

/// PCM sample rate the streaming wire format and whisper both require.
pub const SAMPLE_RATE: usize = 16_000;

/// Windows below this RMS are treated as silence and never transcribed.
///
/// 0.001 is -60 dBFS: above the noise floor of a browser-suppressed mic
/// stream, far below conversational speech (typically 0.02 and up).
const SILENCE_RMS: f64 = 0.001;

/// Minimum audio the interim loop bothers to transcribe; shorter fragments
/// decode to garbage often enough that gating them is cheaper than filtering
/// their output.
pub const MIN_WINDOW_SAMPLES: usize = SAMPLE_RATE / 2;

/// Maximum conditioning prompt handed to the final pass, in chars. Whisper
/// keeps at most half its text context for the prompt (224 tokens), and
/// four chars per token is a conservative English estimate; the tail of the
/// accumulated transcript is what matters for continuity, so the cap trims
/// from the front.
const MAX_PROMPT_CHARS: usize = 800;

/// Whisper's prompt budget in tokens: half the text context
/// (`whisper_n_text_ctx / 2`). A prompt longer than this is truncated by
/// whisper.cpp from the front, which would silently drop a glossary
/// prefix, so prompts are fitted to the budget before being set.
const MAX_PROMPT_TOKENS: usize = 224;

/// Token budget for the glossary on the final-pass worker; the rest of the
/// prompt budget is reserved for the segment-conditioning transcript. The
/// interim worker passes no transcript and fits its glossary to the full
/// budget.
const GLOSSARY_TOKEN_BUDGET: usize = MAX_PROMPT_TOKENS / 2;

/// Root-mean-square amplitude of a PCM buffer.
#[expect(
    clippy::cast_precision_loss,
    reason = "audio buffers are far below 2^53 samples"
)]
fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (energy / samples.len() as f64).sqrt()
}

/// Returns true when the buffer is quiet enough that whisper would
/// hallucinate rather than transcribe.
#[must_use]
pub fn is_silence(samples: &[f32]) -> bool {
    rms(samples) < SILENCE_RMS
}

/// Returns the trailing `window` samples of `buffer`, or the whole buffer
/// when it is shorter than the window.
#[must_use]
pub fn tail(buffer: &[f32], window: usize) -> &[f32] {
    &buffer[buffer.len().saturating_sub(window)..]
}

/// Shared fixtures for the transcription tests: a small GGML whisper model
/// and a 16 kHz mono WAV of known speech, both downloaded out of band (the
/// URLs are recorded in the design log) and gitignored. Gated on the
/// `test-fixtures` feature - which the crate's own dev-dependency enables
/// for every test build - rather than `cfg(test)`, so consumers'
/// integration-test binaries reuse these through their own fixture
/// re-exports instead of duplicating them.
// An `allow` rather than an `expect`: whether the lint fires here depends
// on the build's cfg permutation (clippy suppresses expect_used inside
// test-cfg'd code on its own), so an expectation would be unfulfilled in
// some builds and fail the -D warnings gate.
#[cfg(feature = "test-fixtures")]
#[doc(hidden)]
#[allow(
    clippy::expect_used,
    reason = "test fixtures fail by panicking with the invariant named"
)]
pub mod fixtures {
    use std::path::{Path, PathBuf};

    /// The directory holding the downloaded fixtures.
    #[must_use]
    pub fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// Path to the test model, `ggml-tiny.en.bin`.
    #[must_use]
    pub fn model_path() -> PathBuf {
        fixture_dir().join("ggml-tiny.en.bin")
    }

    /// Path to the test model, panicking with download instructions when it
    /// has not been fetched.
    ///
    /// # Panics
    /// Panics when the model file has not been downloaded, naming the URL
    /// and the destination directory.
    #[must_use]
    pub fn require_model() -> PathBuf {
        let path =
            std::env::var_os("PROMPTFORGE_WHISPER_MODEL").map_or_else(model_path, PathBuf::from);
        assert!(
            path.is_file(),
            "test model missing: download ggml-tiny.en.bin from \
             https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin \
             into {}",
            fixture_dir().display()
        );
        path
    }

    /// Path to the packaged whisper.cpp shared-library test fixture.
    ///
    /// # Panics
    /// Panics when `PROMPTFORGE_WHISPER_LIBRARY` is unset or does not name a
    /// file.
    #[must_use]
    pub fn require_library() -> PathBuf {
        let path = std::env::var_os("PROMPTFORGE_WHISPER_LIBRARY")
            .map(PathBuf::from)
            .unwrap_or_default();
        assert!(
            path.is_file(),
            "set PROMPTFORGE_WHISPER_LIBRARY to the packaged whisper shared library"
        );
        path
    }

    /// Loads the packaged whisper.cpp test library.
    ///
    /// # Panics
    /// Panics when the fixture is absent or the platform loader rejects it.
    #[must_use]
    pub fn require_loaded_library() -> gateway_whisper_ffi::WhisperLibrary {
        gateway_whisper_ffi::WhisperLibrary::load(&require_library())
            .expect("whisper test library loads")
    }

    /// Loads the packaged test library and tiny-model context.
    ///
    /// # Panics
    /// Panics when either fixture is absent or whisper rejects the model.
    #[must_use]
    pub fn require_context() -> (
        gateway_whisper_ffi::WhisperLibrary,
        gateway_whisper_ffi::WhisperContext,
    ) {
        let library = require_loaded_library();
        let context = gateway_whisper_ffi::WhisperContext::new(&library, &require_model())
            .expect("fixture model loads");
        (library, context)
    }

    /// Decodes `jfk.wav` (16 kHz mono s16 PCM, "ask not what your country
    /// can do for you") into f32 samples for the wire format.
    ///
    /// # Panics
    /// Panics when the fixture WAV is missing or is not 16 kHz mono s16
    /// PCM.
    #[must_use]
    pub fn jfk_samples() -> Vec<f32> {
        let path = std::env::var_os("PROMPTFORGE_WHISPER_AUDIO")
            .map_or_else(|| fixture_dir().join("jfk.wav"), PathBuf::from);
        let mut reader =
            hound::WavReader::open(&path).expect("jfk.wav fixture exists beside the test model");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000, "fixture must be 16 kHz");
        assert_eq!(spec.channels, 1, "fixture must be mono");
        assert_eq!(spec.bits_per_sample, 16, "fixture must be 16-bit PCM");
        let samples: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .expect("fixture decodes as s16 PCM");
        samples
            .into_iter()
            .map(|sample| f32::from(sample) / 32_768.0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[]).to_bits(), 0.0f64.to_bits());
        assert_eq!(rms(&[0.0; 1600]).to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn rms_of_a_constant_signal_is_its_amplitude() {
        assert!((rms(&[0.5; 100]) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn silence_gate_separates_quiet_from_speech() {
        assert!(is_silence(&[0.0; 1600]));
        assert!(is_silence(&[0.0005; 1600]));
        assert!(!is_silence(&[0.05; 1600]));
    }

    #[test]
    fn tail_returns_the_trailing_window() {
        let buffer: Vec<f32> = (0u8..10).map(f32::from).collect();
        assert_eq!(tail(&buffer, 4), &[6.0, 7.0, 8.0, 9.0]);
        assert_eq!(tail(&buffer, 100), &buffer[..]);
        assert_eq!(tail(&[], 4), &[] as &[f32]);
    }
}
