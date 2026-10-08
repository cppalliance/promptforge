//! Shared take state tracking finalized text, failures, and final outcomes.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gateway_stt_engine::{DetectorError, SpeechDetector, TranscribeError};

use super::agreement::{final_transcript_within_limit, range_guided_suffix_prefix_start};
use super::final_outcome::{
    FinalBoundary, FinalRangeOutcome, FinalRangeResult, assemble_completion,
};
use super::finalization::ClosedRange;
use super::live_prefix::{AnchoredSuffix, LivePrefixSnapshot};
use super::pcm::{RetainedPcmBudget, RollingPcm};
use super::text::append_transcript;
use super::window::{AcceptedHypothesis, ShownHypotheses};
use crate::segment::{ForcedBoundary, Segmenter};
use reconcile::Settlement;

mod reconcile;

#[derive(Debug, Default)]
struct FinalizedState {
    text: String,
    /// The finalized text that final decodes produced, without accepted
    /// interim text standing in for skipped ranges. It conditions the next
    /// final decode, because whisper copies its prompt's style and interim
    /// text often lacks punctuation.
    decoded_text: String,
    failure: Option<Arc<TakeFailure>>,
    samples: u64,
    /// End of the latest range settled with text, from a final decode or
    /// from accepted interim text. An accepted hypothesis that starts before
    /// it repeats text already settled, so it never stands in for a later
    /// skipped range.
    transcribed_samples: u64,
    applied_outcomes: u64,
    outcomes: Vec<FinalRangeOutcome>,
    pending_forced: Option<PendingForced>,
    /// Displayed words after the latest natural final's last word, live only
    /// until the next final outcome.
    anchored: Option<AnchoredSuffix>,
}

/// One typed take failure retained for commit gating and finalization.
///
/// The failure is shared between the take's slot and the session's precommit
/// gating, so it is reference-counted at the boundary.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TakeFailure {
    #[error("final transcript exceeds the 16 KiB window limit")]
    TranscriptLimit,
    #[error("forced final window was not decoded")]
    ForcedWindowNotDecoded,
    #[error("forced final window was not decodable")]
    ForcedWindowNotDecodable,
    #[error("forced final overlap metadata is inconsistent")]
    ForcedOverlapInconsistent,
    #[error("final outcome capacity is reached")]
    OutcomeCapacity,
    #[error("final transcription pipeline exited")]
    PipelineExited,
    #[error("accepted hypothesis capacity is reached")]
    HypothesisCapacity,
    #[error("forced final PCM retirement failed")]
    RetirementFailed,
    #[error("forced final PCM ownership became inconsistent")]
    OwnershipInconsistent,
    #[error("final window audio was released before its decode")]
    FinalTailEvicted,
    #[error("final transcription worker is unavailable")]
    WorkerUnavailable,
    #[error(transparent)]
    Transcribe(#[from] TranscribeError),
    #[error(transparent)]
    Detector(#[from] DetectorError),
    #[cfg(any(test, feature = "test-fixtures"))]
    #[error("{0}")]
    Recorded(String),
}

#[derive(Debug)]
struct PendingForced {
    boundary: ForcedBoundary,
    /// First sample `text` covers: the window's decode start, or its overlap
    /// end when the window followed a predecessor that settled whole.
    text_start: u64,
    text: String,
}

impl PendingForced {
    fn new(boundary: ForcedBoundary, text: String) -> Self {
        Self {
            text_start: boundary.decode_range().start,
            boundary,
            text,
        }
    }

    /// The audio `text` covers, which is what any reader pairing the text
    /// with a range needs, not the whole decode range.
    fn text_range(&self) -> std::ops::Range<u64> {
        self.text_start..self.boundary.decode_range().end
    }
}

#[derive(Debug)]
pub(super) struct TakeState {
    /// Closed ranges waiting for a final queue slot, in audio order. Lock it
    /// before `buffer`.
    pub(super) held: Mutex<VecDeque<ClosedRange>>,
    pub(super) buffer: Mutex<RollingPcm>,
    pub(super) segmenter: Mutex<Segmenter>,
    finalized: Mutex<FinalizedState>,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl Default for TakeState {
    fn default() -> Self {
        Self::with_detector(Box::new(gateway_stt_engine::EnergyDetector))
    }
}

impl TakeState {
    fn new(budget: RetainedPcmBudget, segmenter: Segmenter) -> Self {
        Self {
            held: Mutex::default(),
            buffer: Mutex::new(RollingPcm::new(budget)),
            segmenter: Mutex::new(segmenter),
            finalized: Mutex::new(FinalizedState::default()),
        }
    }

    #[cfg(test)]
    pub(super) fn with_pcm_limit(limit: usize) -> Self {
        Self::new(
            RetainedPcmBudget::with_limit(limit),
            Segmenter::new(Box::new(gateway_stt_engine::EnergyDetector)),
        )
    }

    pub(super) fn with_detector(detector: Box<dyn SpeechDetector>) -> Self {
        Self::new(RetainedPcmBudget::default(), Segmenter::new(detector))
    }

    /// A state that failed with `failure` before classifying anything.
    pub(super) fn failed(failure: TakeFailure) -> Self {
        let state = Self::new(RetainedPcmBudget::default(), Segmenter::without_detector());
        state.record_failure(failure);
        state
    }

    pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Classifies the audio appended since the last call, queueing the
    /// decisions for segment closing only when `closes_segments`.
    ///
    /// # Errors
    /// Returns the detector's error once; no later audio is classified.
    pub(super) fn classify(&self, closes_segments: bool) -> Result<(), DetectorError> {
        let buffer = Self::lock(&self.buffer);
        Self::lock(&self.segmenter).classify(buffer.samples(), buffer.origin(), closes_segments)
    }

    pub(super) fn finalized(&self) -> String {
        Self::lock(&self.finalized).text.clone()
    }

    /// The finalized text that final decodes produced, the history a final
    /// decode is conditioned on.
    ///
    /// This is empty while a forced window is pending. A pending window's text
    /// is not settled, so the settled text ends well before the next window
    /// starts. Whisper treats a prompt as the text just before the audio, and
    /// a prompt that does not touch the audio makes the final pass, which runs
    /// without timestamps, end the decode after a word or two.
    pub(super) fn decoded_text(&self) -> String {
        let state = Self::lock(&self.finalized);
        if state.pending_forced.is_some() {
            return String::new();
        }
        state.decoded_text.clone()
    }

    /// The count of final outcomes applied to the take.
    pub(super) fn applied_outcomes(&self) -> u64 {
        Self::lock(&self.finalized).applied_outcomes
    }

    #[cfg(test)]
    fn finalized_snapshot(&self) -> (String, u64) {
        self.finalized_snapshot_with(|| {})
    }

    #[cfg(test)]
    fn finalized_snapshot_with(&self, synchronized: impl FnOnce()) -> (String, u64) {
        let state = Self::lock(&self.finalized);
        synchronized();
        (state.text.clone(), state.samples)
    }

    pub(super) fn live_prefix_snapshot(&self) -> LivePrefixSnapshot {
        let state = Self::lock(&self.finalized);
        LivePrefixSnapshot::new(
            state.text.clone(),
            state.samples,
            state.transcribed_samples,
            state.applied_outcomes,
            state
                .pending_forced
                .as_ref()
                .map(|pending| (pending.text.clone(), pending.text_range())),
            state.anchored.clone(),
        )
    }

    #[cfg(test)]
    pub(super) fn record_finalized(
        &self,
        result: Result<String, TranscribeError>,
        samples: Option<u64>,
    ) {
        let mut state = Self::lock(&self.finalized);
        state.anchored = None;
        match result {
            Ok(text) if state.failure.is_none() => {
                append_transcript(&mut state.text, &text);
                append_transcript(&mut state.decoded_text, &text);
                if let Some(samples) = samples {
                    state.samples = samples;
                    state.transcribed_samples = samples;
                }
                state.applied_outcomes = state.applied_outcomes.saturating_add(1);
            }
            Err(error) if state.failure.is_none() => {
                state.failure = Some(Arc::new(TakeFailure::from(error)));
            }
            Ok(_) | Err(_) => {}
        }
    }

    pub(super) fn record_final_outcome(
        &self,
        outcome: FinalRangeOutcome,
        accepted: &[AcceptedHypothesis],
        shown: &ShownHypotheses,
    ) {
        let mut state = Self::lock(&self.finalized);
        state.anchored = None;
        if state.failure.is_some() {
            return;
        }
        if matches!(
            &outcome.result,
            FinalRangeResult::Decoded(text) if !final_transcript_within_limit(text)
        ) {
            state.failure = Some(Arc::new(TakeFailure::TranscriptLimit));
            return;
        }
        match outcome.boundary.clone() {
            FinalBoundary::Natural => record_natural_outcome(&mut state, outcome, accepted, shown),
            FinalBoundary::Forced(boundary) => {
                let FinalRangeResult::Decoded(text) = outcome.result else {
                    state.failure = Some(Arc::new(TakeFailure::ForcedWindowNotDecoded));
                    return;
                };
                record_forced_outcome(&mut state, boundary, text, accepted);
            }
        }
        if state.failure.is_none() {
            state.applied_outcomes = state.applied_outcomes.saturating_add(1);
        }
    }

    pub(super) fn record_failure(&self, failure: TakeFailure) {
        let mut state = Self::lock(&self.finalized);
        if state.failure.is_none() {
            state.failure = Some(Arc::new(failure));
        }
    }

    pub(super) fn has_failure(&self) -> bool {
        Self::lock(&self.finalized).failure.is_some()
    }

    pub(super) fn pending_failure(&self) -> Option<Arc<TakeFailure>> {
        Self::lock(&self.finalized).failure.clone()
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(super) fn coverage(&self) -> (u64, Option<std::ops::Range<u64>>, usize) {
        let state = Self::lock(&self.finalized);
        (
            state.samples,
            state.pending_forced.as_ref().map(PendingForced::text_range),
            state.outcomes.len(),
        )
    }

    #[cfg(test)]
    pub(super) fn take_failure(&self) -> Option<Arc<TakeFailure>> {
        Self::lock(&self.finalized).failure.take()
    }

    pub(super) fn completion(
        &self,
        accepted: &[AcceptedHypothesis],
        committed_samples: u64,
    ) -> Result<String, Arc<TakeFailure>> {
        let mut state = Self::lock(&self.finalized);
        if state.failure.is_none() && !flush_pending_forced(&mut state, accepted) {
            state.failure = Some(Arc::new(TakeFailure::OutcomeCapacity));
        }
        settle_skipped(&mut state, accepted, false);
        match state.failure.take() {
            Some(failure) => Err(failure),
            None if state.outcomes.is_empty() => Ok(state.text.clone()),
            None => {
                let suffix = assemble_completion(&state.outcomes, accepted, committed_samples);
                let mut transcript = state.text.clone();
                append_transcript(&mut transcript, &suffix);
                state.samples = committed_samples;
                Ok(transcript)
            }
        }
    }
}

fn record_natural_outcome(
    state: &mut FinalizedState,
    outcome: FinalRangeOutcome,
    accepted: &[AcceptedHypothesis],
    shown: &ShownHypotheses,
) {
    let flushed = flush_pending_forced(state, accepted);
    debug_assert!(flushed);
    match &outcome.result {
        FinalRangeResult::Decoded(text) => {
            settle_skipped(state, accepted, false);
            reconcile::rewrite_natural(state, text, outcome.range.end, shown);
        }
        FinalRangeResult::Skipped(_) => {
            if let Some(previous) = state.outcomes.last_mut() {
                if outcome.range.start <= previous.range.end {
                    previous.range.end = previous.range.end.max(outcome.range.end);
                } else {
                    settle_skipped(state, accepted, false);
                    state.outcomes.push(outcome);
                }
            } else {
                state.outcomes.push(outcome);
            }
            settle_skipped(state, accepted, true);
        }
    }
}

fn record_forced_outcome(
    state: &mut FinalizedState,
    boundary: ForcedBoundary,
    text: String,
    accepted: &[AcceptedHypothesis],
) {
    let Some(overlap) = boundary.overlap() else {
        if state.pending_forced.is_some() {
            state.failure = Some(Arc::new(TakeFailure::ForcedOverlapInconsistent));
            return;
        }
        state.pending_forced = Some(PendingForced::new(boundary, text));
        return;
    };
    let Some(previous) = state.pending_forced.take() else {
        state.failure = Some(Arc::new(TakeFailure::ForcedOverlapInconsistent));
        return;
    };
    if previous.boundary.decode_range().end != overlap.end
        || boundary.new_audio().start != overlap.end
    {
        state.pending_forced = Some(previous);
        state.failure = Some(Arc::new(TakeFailure::ForcedOverlapInconsistent));
        return;
    }
    let previous_range = previous.text_range();
    let current_range = boundary.decode_range();
    let settlement = match range_guided_suffix_prefix_start(
        &previous.text,
        previous_range.clone(),
        &text,
        current_range.clone(),
        overlap.clone(),
    ) {
        Ok(prefix_end) => Some(Settlement::Prefix(prefix_end)),
        Err(failure) => reconcile::estimate(
            &previous.text,
            &previous_range,
            &text,
            &current_range,
            &overlap,
            &failure,
        ),
    };
    let Some(settlement) = settlement else {
        state.pending_forced = Some(previous);
        state.failure = Some(Arc::new(TakeFailure::ForcedOverlapInconsistent));
        return;
    };
    settle_skipped(state, accepted, false);
    state.pending_forced = Some(match settlement {
        Settlement::Prefix(prefix_end) => {
            settle_decoded(
                state,
                previous_range.start..overlap.start,
                previous.text[..prefix_end].trim_end(),
            );
            PendingForced::new(boundary, text)
        }
        Settlement::SparseSuccessor(successor_start) => {
            settle_decoded(state, previous_range, &previous.text);
            let mut pending =
                PendingForced::new(boundary, text[successor_start..].trim_start().to_owned());
            pending.text_start = overlap.end;
            pending
        }
    });
}

fn flush_pending_forced(state: &mut FinalizedState, accepted: &[AcceptedHypothesis]) -> bool {
    let Some(pending) = state.pending_forced.take() else {
        return true;
    };
    settle_skipped(state, accepted, false);
    settle_decoded(state, pending.text_range(), &pending.text);
    true
}

fn settle_decoded(state: &mut FinalizedState, range: std::ops::Range<u64>, text: &str) {
    append_transcript(&mut state.text, text);
    append_transcript(&mut state.decoded_text, text);
    state.samples = range.end;
    state.transcribed_samples = range.end;
}

fn settle_skipped(
    state: &mut FinalizedState,
    accepted: &[AcceptedHypothesis],
    retain_partial: bool,
) {
    let Some(coverage) = state.outcomes.first().map(|outcome| {
        let end = state
            .outcomes
            .last()
            .map_or(outcome.range.end, |last| last.range.end);
        outcome.range.start..end
    }) else {
        return;
    };
    let candidates = accepted
        .iter()
        .filter(|candidate| {
            let range = candidate.range();
            range.end > state.samples && range.start >= state.transcribed_samples
        })
        .cloned()
        .collect::<Vec<_>>();
    let exact = candidates.iter().find(|candidate| {
        let range = candidate.range();
        coverage.start <= state.samples.max(range.start)
            && coverage.end >= range.end
            && range.end > state.samples
    });
    if let Some(candidate) = exact {
        append_transcript(&mut state.text, candidate.text());
        state.samples = coverage.end;
        state.transcribed_samples = coverage.end;
        state.outcomes.clear();
        return;
    }
    let can_extend_to_candidate = retain_partial
        && candidates.iter().any(|candidate| {
            let range = candidate.range();
            coverage.start <= state.samples.max(range.start)
                && range.start < coverage.end
                && range.end > coverage.end
        });
    if !can_extend_to_candidate {
        state.samples = coverage.end;
        state.outcomes.clear();
    }
}

#[cfg(test)]
mod alignment_tests;

#[cfg(test)]
mod tests;
