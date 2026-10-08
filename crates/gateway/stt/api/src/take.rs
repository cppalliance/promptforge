//! Per-take speech state and finalization ownership.

use std::sync::Arc;
use std::sync::Mutex;

use gateway_stt_engine::DetectorError;
#[cfg(any(test, feature = "test-fixtures"))]
use gateway_stt_engine::SpeechDetector;
#[cfg(test)]
use gateway_stt_engine::TranscribeError;

use crate::audio::AudioError;
use crate::generation::GenerationLease;

mod agreement;
mod final_decode;
mod final_outcome;
mod finalization;
mod interim;
mod live_prefix;
mod pcm;
mod state;
mod text;
mod window;

pub(crate) use agreement::token_spans;
#[cfg(test)]
use finalization::{FINAL_SEGMENT_CAPACITY, FinalCommand, FinalSegmentOwner, run_final_pipeline};
use finalization::{FinalPipeline, append_releasing, spawn_final_pipeline};
pub(crate) use interim::SPEECH_TAIL_SAMPLES;
pub(crate) use interim::{FinalizedRange, InterimSnapshot};
#[cfg(test)]
use pcm::PcmBudgetProbe;
pub(crate) use state::TakeFailure;
use state::TakeState;
use window::WholeWindowState;

#[cfg(feature = "test-fixtures")]
pub(crate) struct TakeMetrics {
    pub(crate) retained_samples: usize,
    pub(crate) finalized_samples: u64,
    pub(crate) unresolved_final: Option<std::ops::Range<u64>>,
    pub(crate) pending_final_segments: usize,
    pub(crate) pending_final_outcomes: usize,
    pub(crate) retained_hypotheses: usize,
}

/// All mutable and immutable state belonging to one speech take.
#[derive(Debug)]
pub(crate) struct Take {
    guidance: Arc<[String]>,
    state: Arc<TakeState>,
    whole_window: Arc<Mutex<WholeWindowState>>,
    final_pipeline: Option<FinalPipeline>,
}

impl Take {
    /// A take that classifies with its generation's Silero detector.
    ///
    /// # Errors
    /// Returns the [`DetectorError`] when the detector does not open, after
    /// logging it; the take does not start.
    pub(crate) fn new(
        guidance: Vec<String>,
        engine: GenerationLease,
    ) -> Result<Self, DetectorError> {
        let detector = engine.speech_detector().inspect_err(|error| {
            tracing::warn!(%error, "the take's Silero speech detector did not open; the take fails");
        })?;
        Ok(Self::with_state(
            guidance,
            Some(engine),
            TakeState::with_detector(detector),
        ))
    }

    /// A take that failed with `failure` before it started: it classifies
    /// nothing and runs no final pass.
    pub(crate) fn failed(guidance: Vec<String>, failure: TakeFailure) -> Self {
        Self::with_state(guidance, None, TakeState::failed(failure))
    }

    fn with_state(
        guidance: Vec<String>,
        engine: Option<GenerationLease>,
        state: TakeState,
    ) -> Self {
        let guidance = Arc::<[String]>::from(guidance);
        let state = Arc::new(state);
        let whole_window = Arc::new(Mutex::new(WholeWindowState::default()));
        let final_pipeline = engine
            .filter(GenerationLease::has_final_pass)
            .map(|engine| {
                spawn_final_pipeline(
                    engine,
                    Arc::clone(&guidance),
                    Arc::clone(&state),
                    Arc::clone(&whole_window),
                )
            });
        Self {
            guidance,
            state,
            whole_window,
            final_pipeline,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_pcm_limit(
        guidance: Vec<String>,
        engine: Option<GenerationLease>,
        limit: usize,
    ) -> Self {
        Self::with_state(guidance, engine, TakeState::with_pcm_limit(limit))
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn with_detector(
        guidance: Vec<String>,
        engine: Option<GenerationLease>,
        detector: Box<dyn SpeechDetector>,
    ) -> Self {
        Self::with_state(guidance, engine, TakeState::with_detector(detector))
    }

    /// A take without a generation, which detects speech by loudness.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn without_final(guidance: Vec<String>) -> Self {
        Self::with_state(guidance, None, TakeState::default())
    }

    pub(crate) fn guidance(&self) -> &[String] {
        &self.guidance
    }

    /// Appends `samples` and classifies them. A detector error is logged and
    /// fails the take; the text finalized before it stays.
    pub(crate) fn append(&self, samples: Vec<f32>) -> Result<(), AudioError> {
        append_releasing(&self.state, samples)?;
        if let Err(error) = self.state.classify(self.final_pipeline.is_some()) {
            tracing::warn!(%error, "the take's Silero speech detector failed; the take fails");
            self.state.record_failure(TakeFailure::Detector(error));
        }
        Ok(())
    }

    pub(crate) fn submit_closed_segments(&self) {
        if let Some(pipeline) = &self.final_pipeline {
            pipeline.submit_closed_segments(&self.state);
        }
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    fn consumed(&self) -> u64 {
        TakeState::lock(&self.state.segmenter).consumed()
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn uncommitted_snapshot(&self, window_samples: usize) -> Vec<f32> {
        let consumed = self.consumed();
        let buffer = TakeState::lock(&self.state.buffer);
        buffer.snapshot_from(consumed, window_samples)
    }

    pub(crate) fn finalized(&self) -> String {
        self.state.finalized()
    }

    /// The absolute sample range of audio the take still holds, for logs.
    pub(crate) fn retained_range(&self) -> std::ops::Range<u64> {
        let buffer = TakeState::lock(&self.state.buffer);
        buffer.origin()..buffer.end()
    }

    /// `_word_ends` holds where each word of `hypothesis` ends, in samples
    /// from `window_start`, or nothing when the decode timed no words. A tail
    /// the fast pass repeated from the silence after speech is cut first, and
    /// a hypothesis left empty changes nothing, like an empty decode. The
    /// finalized range comes from the same live prefix as the snapshot's
    /// finalized part. An accepted hypothesis also tells the segmenter
    /// whether it ends a sentence.
    pub(crate) fn next_window_snapshot(
        &self,
        hypothesis: &str,
        _word_ends: &[u64],
        segment_start: u64,
        window_start: u64,
        window_end: u64,
    ) -> Option<(InterimSnapshot, FinalizedRange)> {
        let live_prefix = self.state.live_prefix_snapshot();
        let speech = TakeState::lock(&self.state.segmenter).speech_before(window_end);
        let mut window = TakeState::lock(&self.whole_window);
        let spoken = speech.map_or_else(
            || hypothesis.into(),
            |speech| window.spoken(&live_prefix, window_start, hypothesis, speech),
        );
        let hypothesis = spoken.text();
        if hypothesis.is_empty() {
            return None;
        }
        let update = window.try_next(
            &live_prefix,
            segment_start,
            window_start,
            window_end,
            spoken,
        );
        drop(window);
        let Ok(snapshot) = update else {
            self.state.record_failure(TakeFailure::HypothesisCapacity);
            return None;
        };
        let snapshot = snapshot?;
        TakeState::lock(&self.state.segmenter)
            .set_sentence_end(segment_start, ends_sentence(hypothesis));
        Some((snapshot, live_prefix.finalized_range()))
    }

    /// The snapshot recomposed without a new hypothesis once final outcomes
    /// have landed beyond the first `seen`, or `None` while none has.
    pub(crate) fn refreshed_snapshot(
        &self,
        seen: u64,
    ) -> Option<(InterimSnapshot, FinalizedRange)> {
        if self.state.applied_outcomes() == seen {
            return None;
        }
        let live_prefix = self.state.live_prefix_snapshot();
        let snapshot = TakeState::lock(&self.whole_window).refresh(&live_prefix);
        Some((snapshot, live_prefix.finalized_range()))
    }

    #[cfg(test)]
    fn record_finalized(&self, result: Result<String, TranscribeError>) {
        self.state.record_finalized(result, None);
    }

    #[cfg(test)]
    pub(crate) fn record_finalized_through(&self, text: &str, samples: Option<u64>) {
        self.state.record_finalized(Ok(text.to_owned()), samples);
    }

    pub(crate) fn record_failure(&self, failure: TakeFailure) {
        self.state.record_failure(failure);
    }

    pub(crate) fn pending_failure(&self) -> Option<Arc<TakeFailure>> {
        self.state.pending_failure()
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn metrics(&self) -> TakeMetrics {
        let retained_samples = TakeState::lock(&self.state.buffer).retained_samples();
        let (finalized_samples, unresolved_final, pending_final_outcomes) = self.state.coverage();
        let retained_hypotheses = TakeState::lock(&self.whole_window).retained_hypothesis_count();
        TakeMetrics {
            retained_samples,
            finalized_samples,
            unresolved_final,
            pending_final_segments: self.pending_final_segments(),
            pending_final_outcomes,
            retained_hypotheses,
        }
    }

    /// Every speech run the take's detector has heard.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn speech_runs(&self) -> Vec<std::ops::Range<u64>> {
        TakeState::lock(&self.state.segmenter)
            .speech_runs()
            .to_vec()
    }

    #[cfg(test)]
    pub(crate) fn pcm_budget_probe(&self) -> PcmBudgetProbe {
        TakeState::lock(&self.state.buffer).budget_probe()
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn pending_final_segments(&self) -> usize {
        self.final_pipeline
            .as_ref()
            .map_or(0, FinalPipeline::pending_segments)
    }

    #[cfg(test)]
    fn take_failure(&self) -> Option<Arc<TakeFailure>> {
        self.state.take_failure()
    }

    pub(crate) fn finalization(&self) -> Option<finalization::TakeFinalization> {
        let pipeline = self.final_pipeline.as_ref()?;
        let committed_samples = TakeState::lock(&self.state.buffer).end();
        let accepted = TakeState::lock(&self.whole_window).accepted_hypotheses(committed_samples);
        Some(pipeline.finalization(committed_samples, accepted))
    }
}

/// Whether `text` ends in sentence-final punctuation.
fn ends_sentence(text: &str) -> bool {
    text.trim_end().ends_with(['.', '?', '!'])
}

#[cfg(test)]
mod detector_tests;

#[cfg(test)]
mod tests;
