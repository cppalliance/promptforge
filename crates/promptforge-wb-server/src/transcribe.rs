//! Whisper transcription on a dedicated worker thread.
//!
//! [`VoiceEngine`] owns one worker thread holding the whisper context and
//! state; callers hand it owned sample buffers through a channel and await
//! the transcript on a oneshot, so the blocking CPU-bound inference never
//! touches the tokio executor. The pure helpers ([`rms`], [`is_silence`],
//! [`tail`]) are the session's silence gate: whisper hallucinates plausible
//! text on silent input, so quiet windows are never sent to the model.

use std::path::{Path, PathBuf};
use std::time::Duration;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::config::VoiceConfig;

/// PCM sample rate the voice wire format and whisper both require.
pub(crate) const SAMPLE_RATE: usize = 16_000;

/// Windows below this RMS are treated as silence and never transcribed.
///
/// 0.001 is -60 dBFS: above the noise floor of a browser-suppressed mic
/// stream, far below conversational speech (typically 0.02 and up).
pub(crate) const SILENCE_RMS: f64 = 0.001;

/// Minimum audio the interim loop bothers to transcribe; shorter fragments
/// decode to garbage often enough that gating them is cheaper than filtering
/// their output.
pub(crate) const MIN_WINDOW_SAMPLES: usize = SAMPLE_RATE / 2;

/// Root-mean-square amplitude of a PCM buffer.
#[expect(
    clippy::cast_precision_loss,
    reason = "audio buffers are far below 2^53 samples"
)]
pub(crate) fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (energy / samples.len() as f64).sqrt()
}

/// Returns true when the buffer is quiet enough that whisper would
/// hallucinate rather than transcribe.
pub(crate) fn is_silence(samples: &[f32]) -> bool {
    rms(samples) < SILENCE_RMS
}

/// Returns the trailing `window` samples of `buffer`, or the whole buffer
/// when it is shorter than the window.
pub(crate) fn tail(buffer: &[f32], window: usize) -> &[f32] {
    &buffer[buffer.len().saturating_sub(window)..]
}

/// The per-server voice engine: the whisper worker plus the interim loop's
/// window and cadence, built once at startup from `[voice]` in
/// `workbench.toml`.
#[derive(Debug)]
pub(crate) struct VoiceEngine {
    transcriber: Transcriber,
    window_samples: usize,
    interval: Duration,
}

impl VoiceEngine {
    /// Loads the interim model onto a fresh worker thread.
    ///
    /// # Errors
    /// Returns [`TranscribeError::InvalidConfig`] when the window or interval
    /// is zero, [`TranscribeError::LoadModel`] when the model file cannot be
    /// loaded, and [`TranscribeError::SpawnWorker`] when the worker thread
    /// cannot be started.
    pub(crate) fn new(config: &VoiceConfig) -> Result<Self, TranscribeError> {
        if config.window_seconds == 0 {
            return Err(TranscribeError::InvalidConfig(
                "voice.window_seconds must be at least 1".to_string(),
            ));
        }
        if config.interval_ms == 0 {
            return Err(TranscribeError::InvalidConfig(
                "voice.interval_ms must be at least 1".to_string(),
            ));
        }
        let window_seconds = usize::try_from(config.window_seconds).map_err(|_| {
            TranscribeError::InvalidConfig("voice.window_seconds is too large".to_string())
        })?;
        let Some(window_samples) = window_seconds.checked_mul(SAMPLE_RATE) else {
            return Err(TranscribeError::InvalidConfig(
                "voice.window_seconds is too large".to_string(),
            ));
        };
        let transcriber = Transcriber::load(&config.interim_model)?;
        Ok(Self {
            transcriber,
            window_samples,
            interval: Duration::from_millis(config.interval_ms),
        })
    }

    /// Samples in the sliding interim window.
    pub(crate) fn window_samples(&self) -> usize {
        self.window_samples
    }

    /// Cadence of the interim loop.
    pub(crate) fn interval(&self) -> Duration {
        self.interval
    }

    /// Transcribes one 16 kHz mono f32 buffer, returning the trimmed text.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio and [`TranscribeError::WorkerGone`] when the worker thread has
    /// exited.
    pub(crate) async fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        self.transcriber.transcribe(samples).await
    }
}

/// One transcription request handed to the worker thread.
struct Job {
    samples: Vec<f32>,
    reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
}

/// Handle to the whisper worker thread.
#[derive(Debug)]
pub(crate) struct Transcriber {
    job_tx: std::sync::mpsc::Sender<Job>,
}

impl Transcriber {
    /// Spawns the worker thread and blocks until the model is loaded or the
    /// load fails.
    ///
    /// # Errors
    /// Returns [`TranscribeError::LoadModel`] when the model file cannot be
    /// loaded and [`TranscribeError::SpawnWorker`] when the thread cannot be
    /// started.
    fn load(model_path: &Path) -> Result<Self, TranscribeError> {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let path = model_path.to_path_buf();
        std::thread::Builder::new()
            .name("whisper-transcribe".to_string())
            .spawn(move || worker_loop(&path, &job_rx, &init_tx))
            .map_err(TranscribeError::SpawnWorker)?;
        init_rx.recv().map_err(|_| TranscribeError::WorkerGone)??;
        Ok(Self { job_tx })
    }

    /// Queues `samples` for transcription and awaits the trimmed text.
    async fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        self.job_tx
            .send(Job { samples, reply })
            .map_err(|_| TranscribeError::WorkerGone)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }
}

/// The worker thread's body: load the model, then transcribe jobs in arrival
/// order until every sender is dropped.
fn worker_loop(
    path: &Path,
    job_rx: &std::sync::mpsc::Receiver<Job>,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) {
    let loaded = WhisperContext::new_with_params(path, WhisperContextParameters::default())
        .map_err(|source| TranscribeError::LoadModel {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
        .and_then(|ctx| {
            ctx.create_state()
                .map(|state| (ctx, state))
                .map_err(|source| TranscribeError::LoadModel {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                })
        });
    // `Transcriber::load` blocks on the init channel until this send (or the
    // worker's exit) completes, so the send can only fail if the spawner is
    // already gone; either way there is nobody left to report to.
    let (_ctx, mut state) = match loaded {
        Ok(pair) => {
            let _ = init_tx.send(Ok(()));
            pair
        }
        Err(error) => {
            let _ = init_tx.send(Err(error));
            return;
        }
    };
    while let Ok(job) = job_rx.recv() {
        // The receiver may be gone (session closed mid-pass); the transcript
        // is computed anyway and the send failure ignored.
        let _ = job
            .reply
            .send(transcribe_blocking(&mut state, &job.samples));
    }
}

/// Runs one blocking whisper pass over `samples` and concatenates the
/// segments.
fn transcribe_blocking(
    state: &mut whisper_rs::WhisperState,
    samples: &[f32],
) -> Result<String, TranscribeError> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_translate(false);
    // Each window stands alone: past output must not prime the decoder, or
    // the sliding loop compounds its own hallucinations.
    params.set_no_context(true);
    params.set_single_segment(true);
    params.set_no_timestamps(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    state
        .full(params, samples)
        .map_err(|source| TranscribeError::Inference(Box::new(source)))?;
    let mut text = String::new();
    for segment in state.as_iter() {
        let piece = segment
            .to_str_lossy()
            .map_err(|source| TranscribeError::Inference(Box::new(source)))?;
        text.push_str(&piece);
    }
    Ok(text.trim().to_string())
}

/// A voice-engine construction or transcription failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TranscribeError {
    /// The whisper model file could not be loaded.
    #[non_exhaustive]
    #[error("load whisper model {}", path.display())]
    LoadModel {
        /// The model path that failed to load.
        path: PathBuf,
        /// The underlying whisper.cpp error, boxed to hide the dependency.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The transcription worker thread could not be started.
    #[non_exhaustive]
    #[error("spawn transcription worker")]
    SpawnWorker(#[source] std::io::Error),

    /// The model rejected an audio window.
    #[non_exhaustive]
    #[error("transcribe audio window")]
    Inference(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The transcription worker exited while requests were in flight.
    #[non_exhaustive]
    #[error("transcription worker exited")]
    WorkerGone,

    /// The `[voice]` configuration is invalid.
    #[non_exhaustive]
    #[error("invalid voice configuration: {0}")]
    InvalidConfig(String),
}

/// Shared fixtures for the transcription tests: a small GGML whisper model
/// and a 16 kHz mono WAV of known speech, both downloaded out of band (the
/// URLs are recorded in the design log) and gitignored.
#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::{Path, PathBuf};

    /// The directory holding the downloaded fixtures.
    pub(crate) fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    /// Path to the test model, `ggml-tiny.en.bin`.
    pub(crate) fn model_path() -> PathBuf {
        fixture_dir().join("ggml-tiny.en.bin")
    }

    /// Path to the test model, panicking with download instructions when it
    /// has not been fetched.
    pub(crate) fn require_model() -> PathBuf {
        let path = model_path();
        assert!(
            path.is_file(),
            "test model missing: download ggml-tiny.en.bin from \
             https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin \
             into {}",
            fixture_dir().display()
        );
        path
    }

    /// Decodes `jfk.wav` (16 kHz mono s16 PCM, "ask not what your country
    /// can do for you") into f32 samples for the wire format.
    pub(crate) fn jfk_samples() -> Vec<f32> {
        let path = fixture_dir().join("jfk.wav");
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
        let mut floats = vec![0.0; samples.len()];
        whisper_rs::convert_integer_to_float_audio(&samples, &mut floats)
            .expect("s16 to f32 conversion cannot fail");
        floats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::transcribe::fixtures;

    #[tokio::test]
    async fn transcribes_known_speech_fixture() {
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            window_seconds: 12,
            ..VoiceConfig::default()
        };
        let engine = VoiceEngine::new(&config).expect("engine loads the fixture model");
        let text = engine
            .transcribe(fixtures::jfk_samples())
            .await
            .expect("transcription succeeds");
        assert!(
            text.to_lowercase().contains("country"),
            "transcript names the fixture's words: {text:?}"
        );
    }

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

    #[test]
    fn invalid_voice_config_is_rejected() {
        let config = VoiceConfig {
            window_seconds: 0,
            ..VoiceConfig::default()
        };
        let err = VoiceEngine::new(&config).expect_err("zero window must fail");
        assert!(
            matches!(err, TranscribeError::InvalidConfig(_)),
            "expected InvalidConfig, got {err:?}"
        );
    }

    #[test]
    fn missing_model_file_fails_engine_construction() {
        let config = VoiceConfig {
            interim_model: PathBuf::from("definitely-missing-model.bin"),
            ..VoiceConfig::default()
        };
        let err = VoiceEngine::new(&config).expect_err("a missing model must fail");
        assert!(
            matches!(err, TranscribeError::LoadModel { .. }),
            "expected LoadModel, got {err:?}"
        );
        assert!(
            err.to_string().contains("definitely-missing-model.bin"),
            "error names the path: {err}"
        );
    }
}
