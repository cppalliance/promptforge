//! The final-pass worker: background transcription of completed segments.

use std::path::Path;

use gateway_whisper_ffi::{WhisperContext, WhisperLibrary, WhisperState};
use shared_progress::ProgressHandle;

use crate::error::TranscribeError;
use crate::prompt::{final_prompt, fit_glossary};
use crate::worker::{load_state, transcribe_blocking};
use crate::{GLOSSARY_TOKEN_BUDGET, MIN_WINDOW_SAMPLES, is_silence};

/// Final-model decoder state confined to its worker thread.
///
/// Guidance and finalized history arrive on every job. The decoder retains
/// no take identity or transcript between jobs.
#[derive(Debug)]
struct FinalDecoder {
    ctx: WhisperContext,
    state: WhisperState,
}

impl FinalDecoder {
    /// Loads the final model from `path`.
    ///
    /// # Errors
    /// Returns [`TranscribeError::LoadModel`] when the model file cannot be
    /// loaded.
    fn load(
        library: &WhisperLibrary,
        path: &Path,
        progress: Option<&ProgressHandle>,
    ) -> Result<Self, TranscribeError> {
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let Some((ctx, state)) = load_state(library, path, progress, &init_tx) else {
            return match init_rx.recv() {
                Ok(Err(error)) => Err(error),
                // `load_state` reports every outcome on the channel before
                // returning `None`, so a disconnected or Ok(()) result here
                // means the invariant broke, not a new failure mode.
                _ => Err(TranscribeError::WorkerGone),
            };
        };
        Ok(Self { ctx, state })
    }

    /// Executes one decode from only the state carried by this job.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio.
    fn transcribe(
        &mut self,
        samples: &[f32],
        guidance: &[String],
        finalized: &str,
    ) -> Result<String, TranscribeError> {
        if samples.len() < MIN_WINDOW_SAMPLES || is_silence(samples) {
            return Ok(String::new());
        }
        let glossary = fit_glossary(&self.ctx, guidance, GLOSSARY_TOKEN_BUDGET);
        let prompt = final_prompt(&self.ctx, glossary.as_deref(), finalized);
        transcribe_blocking(&mut self.state, samples, Some(&prompt), false)
    }
}

/// A command for the final-pass worker thread.
struct FinalJob {
    samples: Vec<f32>,
    guidance: Vec<String>,
    finalized: String,
    reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
}

/// Handle to the final-pass worker thread: the large model transcribing
/// completed segments in the background while a take records.
#[derive(Debug)]
pub(crate) struct FinalTranscriber {
    job_tx: Option<std::sync::mpsc::Sender<FinalJob>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl FinalTranscriber {
    /// Spawns the worker thread, which prewarms and loads the model and then
    /// reports the load outcome on the returned channel. The caller waits on
    /// the channel, so several workers can load in parallel.
    ///
    /// # Errors
    /// Returns [`TranscribeError::SpawnWorker`] when the thread cannot be
    /// started. A model load failure arrives on the returned channel as
    /// [`TranscribeError::LoadModel`].
    pub(super) fn spawn(
        library: WhisperLibrary,
        model_path: &Path,
        progress: Option<ProgressHandle>,
    ) -> Result<(Self, std::sync::mpsc::Receiver<Result<(), TranscribeError>>), TranscribeError>
    {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<FinalJob>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let path = model_path.to_path_buf();
        let worker = std::thread::Builder::new()
            .name("whisper-final".to_string())
            .spawn(move || {
                final_worker_loop(&library, &path, progress.as_ref(), &job_rx, &init_tx);
            })
            .map_err(TranscribeError::SpawnWorker)?;
        Ok((
            Self {
                job_tx: Some(job_tx),
                worker: Some(worker),
            },
            init_rx,
        ))
    }

    /// Executes one independent final-model decode job.
    pub(super) async fn transcribe(
        &self,
        samples: Vec<f32>,
        guidance: Vec<String>,
        finalized: String,
    ) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let Some(job_tx) = &self.job_tx else {
            return Err(TranscribeError::WorkerGone);
        };
        job_tx
            .send(FinalJob {
                samples,
                guidance,
                finalized,
                reply,
            })
            .map_err(|_| TranscribeError::WorkerGone)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }
}

impl Drop for FinalTranscriber {
    fn drop(&mut self) {
        // Close the queue before joining so the worker drains prior jobs,
        // releases its Whisper context, and cannot overlap a replacement.
        drop(self.job_tx.take());
        if let Some(worker) = self.worker.take() {
            let _ignored = worker.join();
        }
    }
}

/// The final-pass worker's body: load the model, then process takes' jobs in
/// arrival order until every sender is dropped.
fn final_worker_loop(
    library: &WhisperLibrary,
    path: &Path,
    progress: Option<&ProgressHandle>,
    job_rx: &std::sync::mpsc::Receiver<FinalJob>,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) {
    let mut decoder = match FinalDecoder::load(library, path, progress) {
        Ok(decoder) => {
            let _ = init_tx.send(Ok(()));
            decoder
        }
        Err(error) => {
            let _ = init_tx.send(Err(error));
            return;
        }
    };
    while let Ok(job) = job_rx.recv() {
        let result = decoder.transcribe(&job.samples, &job.guidance, &job.finalized);
        if let Err(error) = &result {
            tracing::warn!(%error, "final-model transcription failed");
        }
        let _ = job.reply.send(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::engine::SttEngine;
    use crate::{EngineConfig, SAMPLE_RATE, fixtures};

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_pass_biases_segments_with_the_glossary() {
        let vocabulary: Vec<String> = ["MCP", "GGUF"].map(str::to_string).into();
        let library = fixtures::require_loaded_library();
        let mut decoder = FinalDecoder::load(&library, &fixtures::require_model(), None)
            .expect("final decoder loads the fixture model");
        let first = decoder
            .transcribe(&fixtures::jfk_samples(), &vocabulary, "")
            .expect("segment one transcribes");
        assert!(
            first.to_lowercase().contains("country"),
            "segment one names the fixture's words: {first:?}"
        );
        let second = decoder
            .transcribe(&fixtures::jfk_samples(), &vocabulary, &first)
            .expect("segment two transcribes");
        assert!(
            second.to_lowercase().contains("country"),
            "segment two names the fixture's words: {second:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn final_worker_returns_each_stateless_decode_to_its_caller() {
        let config = EngineConfig {
            library: fixtures::require_library(),
            interim_model: fixtures::require_model(),
            final_model: Some(fixtures::require_model()),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let engine = SttEngine::new(&config).expect("engine loads the fixture model");
        let text = engine
            .transcribe_final(fixtures::jfk_samples(), Vec::new(), String::new())
            .await
            .expect("a final model is configured")
            .expect("the final decode succeeds");
        assert!(
            text.to_lowercase().contains("country"),
            "the decode names the fixture's words: {text:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn a_silent_final_job_returns_empty() {
        let config = EngineConfig {
            library: fixtures::require_library(),
            interim_model: fixtures::require_model(),
            final_model: Some(fixtures::require_model()),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let engine = SttEngine::new(&config).expect("engine loads the fixture model");
        let text = engine
            .transcribe_final(vec![0.0; SAMPLE_RATE], Vec::new(), String::new())
            .await
            .expect("a final model is configured")
            .expect("the final decode succeeds");
        assert!(text.is_empty(), "silence is skipped, not transcribed");
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_decoder_uses_only_the_history_supplied_on_each_job() {
        let library = fixtures::require_loaded_library();
        let mut decoder = FinalDecoder::load(&library, &fixtures::require_model(), None)
            .expect("final decoder loads the fixture model");
        let jfk = fixtures::jfk_samples();

        let first = decoder
            .transcribe(&jfk, &[], "")
            .expect("segment one transcribes");
        let first_lower = first.to_lowercase();
        assert!(
            first_lower.contains("country"),
            "segment one names the fixture's words: {first:?}"
        );
        let second = decoder
            .transcribe(&jfk, &[], &first)
            .expect("segment two transcribes");
        assert!(
            second.to_lowercase().contains("country"),
            "the segment's own text names the fixture's words: {second:?}"
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn independent_final_jobs_do_not_require_a_reset() {
        let library = fixtures::require_loaded_library();
        let mut decoder = FinalDecoder::load(&library, &fixtures::require_model(), None)
            .expect("final decoder loads the fixture model");
        let jfk = fixtures::jfk_samples();

        let first = decoder.transcribe(&jfk, &[], "").expect("first job");
        let second = decoder.transcribe(&jfk, &[], "").expect("second job");
        assert_eq!(second, first, "jobs with equal inputs are independent");
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn one_final_job_cannot_change_another_jobs_history() {
        let library = fixtures::require_loaded_library();
        let mut decoder = FinalDecoder::load(&library, &fixtures::require_model(), None)
            .expect("final decoder loads the fixture model");
        let jfk = fixtures::jfk_samples();
        let prompt_sensitive = &jfk[6 * SAMPLE_RATE..8 * SAMPLE_RATE];
        let control = decoder
            .transcribe(prompt_sensitive, &[], "")
            .expect("preconditioned control job");
        let history = decoder
            .transcribe(&jfk[..4 * SAMPLE_RATE], &[], "")
            .expect("history source job");
        let conditioned = decoder
            .transcribe(prompt_sensitive, &[], &history)
            .expect("conditioned job");
        assert_ne!(
            conditioned, control,
            "the fixture must detect transcript conditioning"
        );
        let standalone = decoder
            .transcribe(prompt_sensitive, &[], "")
            .expect("post-conditioned independent job");
        assert_eq!(
            standalone, control,
            "a prior job's history cannot leak into a stateless decode"
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_decoder_skips_silence() {
        let library = fixtures::require_loaded_library();
        let mut decoder = FinalDecoder::load(&library, &fixtures::require_model(), None)
            .expect("final decoder loads the fixture model");
        let text = decoder
            .transcribe(&vec![0.0; SAMPLE_RATE * 2], &[], "history")
            .expect("silence is skipped, not an error");
        assert!(text.is_empty(), "silence transcribes to nothing");
    }
}
