//! The voice engine driving the interim and final-pass whisper workers.

use std::time::Duration;

use crate::config::VoiceConfig;
use crate::transcribe::SAMPLE_RATE;
use crate::transcribe::error::TranscribeError;
use crate::transcribe::final_pass::FinalTranscriber;
use crate::transcribe::worker::Transcriber;

/// The per-server voice engine: the interim and final-pass whisper workers
/// plus the interim loop's window and cadence, built once at startup from
/// `[voice]` in `workshop.toml`.
#[derive(Debug)]
pub(crate) struct VoiceEngine {
    transcriber: Transcriber,
    final_pass: Option<FinalTranscriber>,
    window_samples: usize,
    interval: Duration,
}

impl VoiceEngine {
    /// Loads the interim model, and the final model when configured, each
    /// onto a fresh worker thread.
    ///
    /// # Errors
    /// Returns [`TranscribeError::InvalidConfig`] when the window or interval
    /// is zero, [`TranscribeError::LoadModel`] when a model file cannot be
    /// loaded, and [`TranscribeError::SpawnWorker`] when a worker thread
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
        let transcriber = Transcriber::load(&config.interim_model, &config.vocabulary)?;
        let final_pass = if config.final_model.as_os_str().is_empty() {
            None
        } else {
            Some(FinalTranscriber::load(
                &config.final_model,
                &config.vocabulary,
            )?)
        };
        Ok(Self {
            transcriber,
            final_pass,
            window_samples,
            interval: Duration::from_millis(config.interval_ms),
        })
    }

    /// Whether the final pass is configured. Segmentation and
    /// crystallization only happen when it is: without it nothing can
    /// crystallize, so the segmenter must not consume audio the interim
    /// model still needs.
    pub(crate) fn has_final_pass(&self) -> bool {
        self.final_pass.is_some()
    }

    /// Whether the final pass is absent. A test seam for the startup
    /// degradation policy, which drops an unsourced missing final model.
    #[cfg(test)]
    pub(crate) fn final_pass_absent_for_test(&self) -> bool {
        !self.has_final_pass()
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

    /// Starts a new take on the final-pass worker, discarding the previous
    /// take's accumulated transcript and installing `on_segment` as the
    /// take's completion channel: each background segment's text is sent on
    /// it as the segment finishes. A no-op without a final model.
    pub(crate) fn final_reset(&self, on_segment: std::sync::mpsc::Sender<String>) {
        if let Some(final_pass) = &self.final_pass {
            final_pass.reset(on_segment);
        }
    }

    /// Queues a completed speech segment for background final-pass
    /// transcription, conditioned on the take's accumulated transcript. A
    /// no-op without a final model.
    pub(crate) fn final_submit(&self, samples: Vec<f32>) {
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
    pub(crate) async fn final_finish(
        &self,
        samples: Vec<f32>,
    ) -> Option<Result<String, TranscribeError>> {
        match &self.final_pass {
            None => None,
            Some(final_pass) => Some(final_pass.finish(samples).await),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    use crate::transcribe::fixtures;

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
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
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn missing_final_model_fails_engine_construction() {
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: PathBuf::from("definitely-missing-final-model.bin"),
            ..VoiceConfig::default()
        };
        let err = VoiceEngine::new(&config).expect_err("a missing final model must fail");
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
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            ..VoiceConfig::default()
        };
        let engine = VoiceEngine::new(&config).expect("engine loads the fixture model");
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
