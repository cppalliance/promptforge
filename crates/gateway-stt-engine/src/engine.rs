//! The STT engine driving the interim and final-pass whisper workers.

use std::path::PathBuf;
use std::time::Duration;

use gateway_whisper_ffi::WhisperLibrary;
use shared_progress::ProgressHandle;

use crate::SAMPLE_RATE;
use crate::error::TranscribeError;
use crate::final_pass::FinalTranscriber;
use crate::worker::Transcriber;

/// Engine construction settings: plain values the host maps from its own
/// configuration type, so the engine never depends back on its host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineConfig {
    /// Path to the provisioned whisper.cpp shared library.
    pub library: PathBuf,
    /// Path to the GGML/GGUF whisper model for interim (streaming)
    /// transcription.
    pub interim_model: PathBuf,
    /// Path to the whisper model for the pipelined final pass over a take.
    /// `None` disables the final pass; the final transcript then comes from
    /// the interim model.
    pub final_model: Option<PathBuf>,
    /// Domain terms whisper is biased toward (for example `MCP`, `GGUF`,
    /// `Lua`), formatted into a glossary conditioning prompt on both
    /// workers. Empty disables biasing.
    pub vocabulary: Vec<String>,
    /// Seconds of trailing audio each interim pass transcribes.
    pub window_seconds: u64,
    /// Milliseconds between interim passes while a take is recording.
    pub interval_ms: u64,
}

/// The STT engine: the interim and final-pass whisper workers plus the
/// interim loop's window and cadence, built once at startup from the host's
/// STT configuration.
#[derive(Debug)]
pub struct SttEngine {
    transcriber: Transcriber,
    final_pass: Option<FinalTranscriber>,
    gpu_available: bool,
    window_samples: usize,
    interval: Duration,
}

impl SttEngine {
    /// Loads the interim model, and the final model when configured, each
    /// onto a fresh worker thread.
    ///
    /// # Errors
    /// Returns [`TranscribeError::InvalidConfig`] when the window or interval
    /// is zero, [`TranscribeError::LoadLibrary`] when the provisioned runtime
    /// cannot be opened, [`TranscribeError::LoadModel`] when a model file
    /// cannot be loaded, and [`TranscribeError::SpawnWorker`] when a worker
    /// thread cannot be started.
    pub fn new(config: &EngineConfig) -> Result<Self, TranscribeError> {
        Self::new_with_progress(config, None)
    }

    /// [`SttEngine::new`] plus progress reporting: `progress` gains one
    /// child per loaded model (`interim`, `final`), each with a byte-counted
    /// `prewarm` leaf and an indeterminate `init` leaf completed when the
    /// whisper context is ready. Both worker threads prewarm and load in
    /// parallel.
    ///
    /// # Errors
    /// Returns [`TranscribeError::InvalidConfig`] when the window or interval
    /// is zero, [`TranscribeError::LoadLibrary`] when the provisioned runtime
    /// cannot be opened, [`TranscribeError::LoadModel`] when a model file
    /// cannot be loaded, and [`TranscribeError::SpawnWorker`] when a worker
    /// thread cannot be started.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the caller hands its leaf handle to the engine, which registers per-model children on it"
    )]
    pub fn new_with_progress(
        config: &EngineConfig,
        progress: Option<ProgressHandle>,
    ) -> Result<Self, TranscribeError> {
        if config.window_seconds == 0 {
            return Err(TranscribeError::InvalidConfig(
                "stt.window_seconds must be at least 1".to_string(),
            ));
        }
        if config.interval_ms == 0 {
            return Err(TranscribeError::InvalidConfig(
                "stt.interval_ms must be at least 1".to_string(),
            ));
        }
        let window_seconds = usize::try_from(config.window_seconds).map_err(|_| {
            TranscribeError::InvalidConfig("stt.window_seconds is too large".to_string())
        })?;
        let Some(window_samples) = window_seconds.checked_mul(SAMPLE_RATE) else {
            return Err(TranscribeError::InvalidConfig(
                "stt.window_seconds is too large".to_string(),
            ));
        };
        require_model_file(&config.interim_model)?;
        if let Some(final_model) = &config.final_model {
            require_model_file(final_model)?;
        }
        let library = WhisperLibrary::load(&config.library).map_err(|source| {
            TranscribeError::LoadLibrary {
                path: config.library.clone(),
                source: Box::new(source),
            }
        })?;
        // Route ggml/whisper C logging into tracing before any context is
        // created, so the `whisper_cpp=warn` filter covers engine startup.
        library.set_log_callback();
        let gpu_available = library.gpu_available().unwrap_or_else(|error| {
            tracing::warn!(%error, "could not inspect whisper GPU support");
            false
        });
        let interim_progress = progress.as_ref().map(|handle| handle.child("interim", 1.0));
        let final_progress = match (&config.final_model, &progress) {
            (Some(_), Some(handle)) => Some(handle.child("final", 1.0)),
            _ => None,
        };
        // Both workers prewarm and load concurrently; the waits below only
        // collect the outcomes, with the interim outcome reported first.
        let (transcriber, interim_init) = Transcriber::spawn(
            library.clone(),
            &config.interim_model,
            &config.vocabulary,
            interim_progress,
        )?;
        let final_spawned = match &config.final_model {
            None => None,
            Some(final_model) => Some(FinalTranscriber::spawn(
                library,
                final_model,
                &config.vocabulary,
                final_progress,
            )?),
        };
        interim_init
            .recv()
            .map_err(|_| TranscribeError::WorkerGone)??;
        let final_pass = match final_spawned {
            None => None,
            Some((final_transcriber, final_init)) => {
                final_init
                    .recv()
                    .map_err(|_| TranscribeError::WorkerGone)??;
                Some(final_transcriber)
            }
        };
        Ok(Self {
            transcriber,
            final_pass,
            gpu_available,
            window_samples,
            interval: Duration::from_millis(config.interval_ms),
        })
    }

    /// Whether the final pass is configured. Segmentation and
    /// crystallization only happen when it is: without it nothing can
    /// crystallize, so the segmenter must not consume audio the interim
    /// model still needs.
    #[must_use]
    pub fn has_final_pass(&self) -> bool {
        self.final_pass.is_some()
    }

    /// Whether the loaded whisper.cpp runtime reports CUDA or Metal support.
    #[must_use]
    pub fn gpu_transcription_available(&self) -> bool {
        self.gpu_available
    }

    /// Whether the final pass is absent. A test seam for the host's startup
    /// degradation policy, which drops an unsourced missing final model.
    #[cfg(feature = "test-fixtures")]
    #[doc(hidden)]
    #[must_use]
    pub fn final_pass_absent_for_test(&self) -> bool {
        !self.has_final_pass()
    }

    /// Samples in the sliding interim window.
    #[must_use]
    pub fn window_samples(&self) -> usize {
        self.window_samples
    }

    /// Cadence of the interim loop.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Transcribes one 16 kHz mono f32 buffer, returning the trimmed text.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio and [`TranscribeError::WorkerGone`] when the worker thread has
    /// exited.
    pub async fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        self.transcriber.transcribe(samples).await
    }

    /// Transcribes one independent buffer with the final model.
    ///
    /// This request does not read or change the active streaming take.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio and [`TranscribeError::WorkerGone`] when the worker exits.
    pub async fn transcribe_final(
        &self,
        samples: Vec<f32>,
    ) -> Option<Result<String, TranscribeError>> {
        match &self.final_pass {
            Some(final_pass) => Some(final_pass.transcribe(samples).await),
            None => None,
        }
    }

    /// Starts a new take on the final-pass worker, discarding the previous
    /// take's accumulated transcript and installing `on_segment` as the
    /// take's completion channel: each background segment's text is sent on
    /// it as the segment finishes. A no-op without a final model.
    pub fn final_reset(&self, on_segment: std::sync::mpsc::Sender<String>) {
        if let Some(final_pass) = &self.final_pass {
            final_pass.reset(on_segment);
        }
    }

    /// Queues a completed speech segment for background final-pass
    /// transcription, conditioned on the take's accumulated transcript. A
    /// no-op without a final model.
    pub fn final_submit(&self, samples: Vec<f32>) {
        if let Some(final_pass) = &self.final_pass {
            final_pass.submit(samples);
        }
    }

    /// Queues the take's unprocessed tail and awaits the tail's own
    /// transcription - not the take's full assembled transcript, which the
    /// session already holds as crystallized segment text. The text is
    /// empty when the tail is silent or too short to decode (the worker
    /// skips those rather than hallucinating). Returns `None` when no
    /// final model is configured and the caller should fall back to the
    /// interim model.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio and [`TranscribeError::WorkerGone`] when the worker thread has
    /// exited.
    pub async fn final_finish(&self, samples: Vec<f32>) -> Option<Result<String, TranscribeError>> {
        match &self.final_pass {
            None => None,
            Some(final_pass) => Some(final_pass.finish(samples).await),
        }
    }
}

fn require_model_file(path: &std::path::Path) -> Result<(), TranscribeError> {
    let metadata = std::fs::metadata(path).map_err(|source| TranscribeError::LoadModel {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(TranscribeError::LoadModel {
            path: path.to_path_buf(),
            source: Box::new(std::io::Error::other("model path is not a file")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixtures;

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn transcribes_known_speech_fixture() {
        let config = EngineConfig {
            library: fixtures::require_library(),
            interim_model: fixtures::require_model(),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let engine = SttEngine::new(&config).expect("engine loads the fixture model");
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
    fn invalid_stt_config_is_rejected() {
        let config = EngineConfig {
            window_seconds: 0,
            ..EngineConfig::default()
        };
        let err = SttEngine::new(&config).expect_err("zero window must fail");
        assert!(
            matches!(err, TranscribeError::InvalidConfig(_)),
            "expected InvalidConfig, got {err:?}"
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn missing_final_model_fails_engine_construction() {
        let config = EngineConfig {
            library: fixtures::require_library(),
            interim_model: fixtures::require_model(),
            final_model: Some(PathBuf::from("definitely-missing-final-model.bin")),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let err = SttEngine::new(&config).expect_err("a missing final model must fail");
        assert!(
            matches!(err, TranscribeError::LoadModel { .. }),
            "expected LoadModel, got {err:?}"
        );
        assert!(
            err.to_string()
                .contains("definitely-missing-final-model.bin"),
            "error names the path: {err}"
        );
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn final_pass_entry_points_are_no_ops_without_a_final_model() {
        let config = EngineConfig {
            library: fixtures::require_library(),
            interim_model: fixtures::require_model(),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let engine = SttEngine::new(&config).expect("engine loads the fixture model");
        let (segment_tx, _segment_rx) = std::sync::mpsc::channel();
        engine.final_reset(segment_tx);
        engine.final_submit(fixtures::jfk_samples());
        assert!(
            engine.final_finish(fixtures::jfk_samples()).await.is_none(),
            "no final model means the caller falls back"
        );
    }

    #[test]
    fn missing_model_file_fails_engine_construction() {
        let config = EngineConfig {
            interim_model: PathBuf::from("definitely-missing-model.bin"),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let err = SttEngine::new(&config).expect_err("a missing model must fail");
        assert!(
            matches!(err, TranscribeError::LoadModel { .. }),
            "expected LoadModel, got {err:?}"
        );
        assert!(
            err.to_string().contains("definitely-missing-model.bin"),
            "error names the path: {err}"
        );
    }

    #[test]
    fn new_with_progress_without_a_handle_behaves_like_new() {
        let config = EngineConfig {
            interim_model: PathBuf::from("definitely-missing-model.bin"),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let err =
            SttEngine::new_with_progress(&config, None).expect_err("a missing model must fail");
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
