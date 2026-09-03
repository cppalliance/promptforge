//! The final-pass worker: background transcription of completed segments.

use std::path::Path;

use shared_progress::ProgressHandle;
use gateway_whisper_ffi::{WhisperContext, WhisperLibrary, WhisperState};

use crate::error::TranscribeError;
use crate::prompt::{final_prompt, fit_glossary};
use crate::worker::{load_state, transcribe_blocking};
use crate::{GLOSSARY_TOKEN_BUDGET, MIN_WINDOW_SAMPLES, is_silence};

/// One take's final-pass state: the large model's whisper context and state
/// plus the take's accumulated transcript, which conditions each new
/// segment so domain vocabulary and phrasing survive segmentation. The
/// glossary prompt (fitted at load from the STT vocabulary) biases every
/// segment toward the configured domain terms.
#[derive(Debug)]
pub(crate) struct FinalPass {
    ctx: WhisperContext,
    state: WhisperState,
    /// The fitted glossary prompt, `None` when no vocabulary is configured.
    glossary: Option<String>,
    /// Every segment transcript so far, joined by single spaces.
    transcript: String,
    /// The conditioning prompt used on the most recent segment, kept so
    /// tests can observe that conditioning actually happened.
    last_prompt: String,
}

impl FinalPass {
    /// Loads the final model from `path` and fits the vocabulary glossary.
    ///
    /// # Errors
    /// Returns [`TranscribeError::LoadModel`] when the model file cannot be
    /// loaded.
    fn load(
        library: &WhisperLibrary,
        path: &Path,
        vocabulary: &[String],
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
        let glossary = fit_glossary(&ctx, vocabulary, GLOSSARY_TOKEN_BUDGET);
        Ok(Self {
            ctx,
            state,
            glossary,
            transcript: String::new(),
            last_prompt: String::new(),
        })
    }

    /// Forgets the previous take's transcript for a new take.
    fn reset(&mut self) {
        self.transcript.clear();
        self.last_prompt.clear();
    }

    /// The conditioning prompt the most recent segment was transcribed with.
    #[cfg(test)]
    pub(crate) fn last_prompt(&self) -> &str {
        &self.last_prompt
    }

    /// The take's accumulated transcript: every segment so far, joined by
    /// single spaces. A test-only observation point for the conditioning
    /// chain; the workers consume only each segment's own text.
    #[cfg(test)]
    pub(crate) fn transcript(&self) -> &str {
        &self.transcript
    }

    /// Transcribes one segment conditioned on the accumulated transcript,
    /// appends the result, and returns the segment's own text. Silent or
    /// tiny fragments are skipped (whisper hallucinates on them): the
    /// accumulated transcript is left unchanged and `None` comes back.
    ///
    /// # Errors
    /// Returns [`TranscribeError::Inference`] when the model rejects the
    /// audio; the accumulated transcript is left unchanged.
    fn transcribe_segment(&mut self, samples: &[f32]) -> Result<Option<String>, TranscribeError> {
        let mut segment = None;
        if samples.len() >= MIN_WINDOW_SAMPLES && !is_silence(samples) {
            let prompt = final_prompt(&self.ctx, self.glossary.as_deref(), &self.transcript);
            let text = transcribe_blocking(&mut self.state, samples, Some(&prompt), false)?;
            if !text.is_empty() {
                if !self.transcript.is_empty() {
                    self.transcript.push(' ');
                }
                self.transcript.push_str(&text);
                segment = Some(text);
            }
            self.last_prompt = prompt;
        }
        Ok(segment)
    }

    /// Transcribes one independent request without reading or changing the
    /// active streaming take.
    fn transcribe_standalone(&mut self, samples: &[f32]) -> Result<String, TranscribeError> {
        if samples.len() < MIN_WINDOW_SAMPLES || is_silence(samples) {
            return Ok(String::new());
        }
        let prompt = final_prompt(&self.ctx, self.glossary.as_deref(), "");
        transcribe_blocking(&mut self.state, samples, Some(&prompt), false)
    }
}

/// A command for the final-pass worker thread.
enum FinalJob {
    /// Start a new take, discarding the accumulated transcript and
    /// installing the take's segment-completion channel.
    Reset {
        on_segment: std::sync::mpsc::Sender<String>,
    },
    /// Transcribe a completed segment (or the closing tail) and reply with
    /// the segment's own text, empty when the fragment was skipped.
    /// `notify` marks a background submit, whose segment text is also sent
    /// on the take's channel; the closing tail reports only through its
    /// reply.
    Segment {
        samples: Vec<f32>,
        reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
        notify: bool,
    },
    /// Transcribe an independent request without touching take state.
    Standalone {
        samples: Vec<f32>,
        reply: tokio::sync::oneshot::Sender<Result<String, TranscribeError>>,
    },
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
        vocabulary: &[String],
        progress: Option<ProgressHandle>,
    ) -> Result<(Self, std::sync::mpsc::Receiver<Result<(), TranscribeError>>), TranscribeError>
    {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<FinalJob>();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let path = model_path.to_path_buf();
        let vocabulary = vocabulary.to_vec();
        let worker = std::thread::Builder::new()
            .name("whisper-final".to_string())
            .spawn(move || {
                final_worker_loop(
                    &library,
                    &path,
                    &vocabulary,
                    progress.as_ref(),
                    &job_rx,
                    &init_tx,
                );
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

    /// Starts a new take, installing `on_segment` as the channel each
    /// background segment's text is reported on. If the worker is gone the
    /// next `finish` reports it.
    pub(super) fn reset(&self, on_segment: std::sync::mpsc::Sender<String>) {
        if let Some(job_tx) = &self.job_tx {
            let _ = job_tx.send(FinalJob::Reset { on_segment });
        }
    }

    /// Queues a completed segment for background transcription; the
    /// segment's text is reported on the take's channel.
    pub(super) fn submit(&self, samples: Vec<f32>) {
        let (reply, _dropped) = tokio::sync::oneshot::channel();
        if let Some(job_tx) = &self.job_tx {
            let _ = job_tx.send(FinalJob::Segment {
                samples,
                reply,
                notify: true,
            });
        }
    }

    /// Queues the take's tail and awaits the tail's own text, empty when
    /// the tail was skipped. Because the channel is FIFO, awaiting this
    /// reply also drains every segment submitted earlier in the take.
    pub(super) async fn finish(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let Some(job_tx) = &self.job_tx else {
            return Err(TranscribeError::WorkerGone);
        };
        job_tx
            .send(FinalJob::Segment {
                samples,
                reply,
                notify: false,
            })
            .map_err(|_| TranscribeError::WorkerGone)?;
        reply_rx.await.map_err(|_| TranscribeError::WorkerGone)?
    }

    /// Transcribes one independent buffer without changing the active take.
    pub(super) async fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeError> {
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let Some(job_tx) = &self.job_tx else {
            return Err(TranscribeError::WorkerGone);
        };
        job_tx
            .send(FinalJob::Standalone { samples, reply })
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
    vocabulary: &[String],
    progress: Option<&ProgressHandle>,
    job_rx: &std::sync::mpsc::Receiver<FinalJob>,
    init_tx: &std::sync::mpsc::SyncSender<Result<(), TranscribeError>>,
) {
    let mut pass = match FinalPass::load(library, path, vocabulary, progress) {
        Ok(pass) => {
            let _ = init_tx.send(Ok(()));
            pass
        }
        Err(error) => {
            let _ = init_tx.send(Err(error));
            return;
        }
    };
    // The current take's completion channel, installed by each `Reset`;
    // FIFO job order guarantees a take's segments all precede the next
    // take's `Reset`, so a segment can never land on the wrong channel.
    let mut on_segment: Option<std::sync::mpsc::Sender<String>> = None;
    while let Ok(job) = job_rx.recv() {
        match job {
            FinalJob::Reset {
                on_segment: channel,
            } => {
                on_segment = Some(channel);
                pass.reset();
            }
            FinalJob::Segment {
                samples,
                reply,
                notify,
            } => {
                let result = pass.transcribe_segment(&samples);
                match &result {
                    Ok(segment) => {
                        if notify && let (Some(channel), Some(text)) = (&on_segment, segment) {
                            // A gone session (socket closed mid-take) is
                            // ordinary; the transcript was computed anyway.
                            if channel.send(text.clone()).is_err() {
                                tracing::debug!("segment completion receiver is gone");
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "final-pass segment transcription failed");
                    }
                }
                // A dropped receiver (a background segment, or a session
                // closed mid-take) is fine: the transcript was computed.
                let _ = reply.send(result.map(Option::unwrap_or_default));
            }
            FinalJob::Standalone { samples, reply } => {
                let result = pass.transcribe_standalone(&samples);
                if let Err(error) = &result {
                    tracing::warn!(%error, "standalone final-model transcription failed");
                }
                let _ = reply.send(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    use crate::engine::SttEngine;
    use crate::{EngineConfig, SAMPLE_RATE, fixtures};

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_pass_biases_segments_with_the_glossary() {
        let vocabulary: Vec<String> = ["MCP", "GGUF"].map(str::to_string).into();
        let library = fixtures::require_loaded_library();
        let mut pass = FinalPass::load(&library, &fixtures::require_model(), &vocabulary, None)
            .expect("final pass loads the fixture model");
        let first = pass
            .transcribe_segment(&fixtures::jfk_samples())
            .expect("segment one transcribes")
            .expect("segment one appended text");
        assert!(
            first.to_lowercase().contains("country"),
            "segment one names the fixture's words: {first:?}"
        );
        assert!(
            pass.last_prompt().starts_with("Glossary: MCP, GGUF."),
            "the first segment was conditioned on the glossary: {:?}",
            pass.last_prompt()
        );
        let second = pass
            .transcribe_segment(&fixtures::jfk_samples())
            .expect("segment two transcribes")
            .expect("segment two appended text");
        assert!(
            second.to_lowercase().contains("country"),
            "segment two names the fixture's words: {second:?}"
        );
        let prompt = pass.last_prompt();
        assert!(
            prompt.starts_with("Glossary: MCP, GGUF. "),
            "the glossary leads the conditioning prompt: {prompt:?}"
        );
        assert!(
            prompt.contains(&first),
            "the transcript follows the glossary: {prompt:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn final_submit_reports_the_segment_on_the_take_channel() {
        let config = EngineConfig {
            library: fixtures::require_library(),
            interim_model: fixtures::require_model(),
            final_model: Some(fixtures::require_model()),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let engine = SttEngine::new(&config).expect("engine loads the fixture model");
        let (segment_tx, segment_rx) = std::sync::mpsc::channel();
        engine.final_reset(segment_tx);
        engine.final_submit(fixtures::jfk_samples());

        // The timeout only bounds a broken pipeline; the tiny fixture
        // model transcribes the clip in seconds.
        let segment = segment_rx
            .recv_timeout(Duration::from_secs(120))
            .expect("the submitted segment's text arrives on the channel");
        assert!(
            segment.to_lowercase().contains("country"),
            "the reported segment names the fixture's words: {segment:?}"
        );

        let tail = engine
            .final_finish(fixtures::jfk_samples())
            .await
            .expect("a final model is configured")
            .expect("the final pass succeeds");
        assert!(
            tail.to_lowercase().contains("country"),
            "the closing tail names the fixture's words: {tail:?}"
        );
        let countries = tail.to_lowercase().matches("country").count();
        assert!(
            countries < 3,
            "the finish returns the tail's text only, not the assembled \
             transcript ({countries} countries): {tail:?}"
        );
        assert!(
            segment_rx.try_recv().is_err(),
            "the closing tail reports only through its reply, not the channel"
        );
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn final_finish_with_a_silent_tail_returns_empty_after_draining() {
        let config = EngineConfig {
            library: fixtures::require_library(),
            interim_model: fixtures::require_model(),
            final_model: Some(fixtures::require_model()),
            window_seconds: 12,
            interval_ms: 500,
            ..EngineConfig::default()
        };
        let engine = SttEngine::new(&config).expect("engine loads the fixture model");
        let (segment_tx, segment_rx) = std::sync::mpsc::channel();
        engine.final_reset(segment_tx);
        engine.final_submit(fixtures::jfk_samples());

        // The tail is pure silence: the worker skips it rather than
        // hallucinating, and the FIFO reply still drains the take's
        // submitted segment first.
        let tail = engine
            .final_finish(vec![0.0; SAMPLE_RATE])
            .await
            .expect("a final model is configured")
            .expect("the final pass succeeds");
        assert!(
            tail.is_empty(),
            "a silent tail is skipped, not transcribed: {tail:?}"
        );
        let segment = segment_rx
            .recv_timeout(Duration::from_secs(120))
            .expect("the submitted segment's text arrives on the channel");
        assert!(
            segment.to_lowercase().contains("country"),
            "the drained segment names the fixture's words: {segment:?}"
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_pass_conditions_each_segment_on_the_accumulated_transcript() {
        let library = fixtures::require_loaded_library();
        let mut pass = FinalPass::load(&library, &fixtures::require_model(), &[], None)
            .expect("final pass loads the fixture model");
        let jfk = fixtures::jfk_samples();

        let first = pass
            .transcribe_segment(&jfk)
            .expect("segment one transcribes")
            .expect("segment one appended text");
        assert!(
            pass.last_prompt().is_empty(),
            "the first segment has nothing to be conditioned on"
        );
        let first_lower = first.to_lowercase();
        assert!(
            first_lower.contains("country"),
            "segment one names the fixture's words: {first:?}"
        );
        let first_countries = first_lower.matches("country").count();
        assert_eq!(
            pass.transcript(),
            first,
            "the accumulated transcript is the first segment's text"
        );

        let second = pass
            .transcribe_segment(&jfk)
            .expect("segment two transcribes")
            .expect("segment two appended text");
        assert_eq!(
            pass.last_prompt(),
            first,
            "segment two was conditioned on the accumulated transcript"
        );
        assert!(
            second.to_lowercase().contains("country"),
            "the segment's own text names the fixture's words: {second:?}"
        );
        let assembled = pass.transcript();
        assert!(
            assembled.starts_with(&first),
            "segment transcripts accumulate in order: {assembled:?}"
        );
        let second_countries = assembled.to_lowercase().matches("country").count();
        assert!(
            second_countries > first_countries,
            "the second segment added its own text: {first_countries} then {second_countries}"
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_pass_reset_forgets_the_accumulated_transcript() {
        let library = fixtures::require_loaded_library();
        let mut pass = FinalPass::load(&library, &fixtures::require_model(), &[], None)
            .expect("final pass loads the fixture model");
        let jfk = fixtures::jfk_samples();

        let first = pass
            .transcribe_segment(&jfk)
            .expect("segment one transcribes")
            .expect("segment one appended text");
        pass.reset();
        let second = pass
            .transcribe_segment(&jfk)
            .expect("segment two transcribes")
            .expect("segment two appended text");
        assert!(
            pass.last_prompt().is_empty(),
            "after reset the next segment has nothing to be conditioned on"
        );
        assert_eq!(
            second, first,
            "a new take's transcript holds only its own segments"
        );
        assert_eq!(
            pass.transcript(),
            second,
            "the accumulated transcript forgot the previous take"
        );
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn standalone_transcription_does_not_change_the_streaming_take() {
        let library = fixtures::require_loaded_library();
        let mut pass = FinalPass::load(&library, &fixtures::require_model(), &[], None)
            .expect("final pass loads the fixture model");
        let jfk = fixtures::jfk_samples();
        let _first = pass
            .transcribe_segment(&jfk)
            .expect("streaming segment transcribes")
            .expect("streaming segment has text");
        let transcript = pass.transcript().to_owned();
        let last_prompt = pass.last_prompt().to_owned();
        let standalone = pass
            .transcribe_standalone(&jfk)
            .expect("standalone request transcribes");
        assert!(standalone.to_lowercase().contains("country"));
        assert_eq!(
            pass.transcript(),
            transcript,
            "request-response transcription cannot change streaming take state"
        );
        assert_eq!(pass.last_prompt(), last_prompt);
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn final_pass_skips_silence_without_touching_the_transcript() {
        let library = fixtures::require_loaded_library();
        let mut pass = FinalPass::load(&library, &fixtures::require_model(), &[], None)
            .expect("final pass loads the fixture model");
        let segment = pass
            .transcribe_segment(&vec![0.0; SAMPLE_RATE * 2])
            .expect("silence is skipped, not an error");
        assert!(segment.is_none(), "a skipped segment reports no text");
        assert!(
            pass.transcript().is_empty(),
            "silence transcribes to nothing"
        );
        assert!(
            pass.last_prompt().is_empty(),
            "a skipped segment records no conditioning"
        );
    }
}
