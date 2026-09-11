use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gateway_stt_engine::TranscribeError;

use super::agreement::{
    final_transcript_within_limit, projected_prefix_end, range_guided_suffix_prefix_start,
};
use super::final_outcome::{
    FinalBoundary, FinalRangeOutcome, FinalRangeResult, assemble_completion,
};
use super::live_prefix::LivePrefixSnapshot;
use super::pcm::{RetainedPcmBudget, RollingPcm};
use super::text::append_transcript;
use super::window::AcceptedHypothesis;
use crate::segment::{ForcedBoundary, Segmenter};

#[derive(Debug, Default)]
struct FinalizedState {
    text: String,
    failure: Option<Arc<TakeFailure>>,
    samples: u64,
    outcomes: Vec<FinalRangeOutcome>,
    pending_forced: Option<PendingForced>,
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
    #[error("final segment capacity is reached")]
    SegmentCapacity,
    #[error("final transcription pipeline exited")]
    PipelineExited,
    #[error("accepted hypothesis capacity is reached")]
    HypothesisCapacity,
    #[error("forced final PCM retirement failed")]
    RetirementFailed,
    #[error("forced final PCM ownership became inconsistent")]
    OwnershipInconsistent,
    #[error("final transcription worker is unavailable")]
    WorkerUnavailable,
    #[error(transparent)]
    Transcribe(#[from] TranscribeError),
    #[cfg(any(test, feature = "test-fixtures"))]
    #[error("{0}")]
    Recorded(String),
}

#[derive(Debug)]
struct PendingForced {
    boundary: ForcedBoundary,
    text: String,
}

#[derive(Debug)]
pub(super) struct TakeState {
    pub(super) buffer: Mutex<RollingPcm>,
    pub(super) segmenter: Mutex<Segmenter>,
    finalized: Mutex<FinalizedState>,
}

impl Default for TakeState {
    fn default() -> Self {
        Self {
            buffer: Mutex::new(RollingPcm::new(RetainedPcmBudget::default())),
            segmenter: Mutex::new(Segmenter::default()),
            finalized: Mutex::new(FinalizedState::default()),
        }
    }
}

impl TakeState {
    #[cfg(test)]
    pub(super) fn with_pcm_limit(limit: usize) -> Self {
        Self {
            buffer: Mutex::new(RollingPcm::new(RetainedPcmBudget::with_limit(limit))),
            segmenter: Mutex::new(Segmenter::default()),
            finalized: Mutex::new(FinalizedState::default()),
        }
    }

    pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn finalized(&self) -> String {
        Self::lock(&self.finalized).text.clone()
    }

    #[cfg(test)]
    pub(super) fn finalized_snapshot(&self) -> (String, u64) {
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
            state
                .pending_forced
                .as_ref()
                .map(|pending| (pending.text.clone(), pending.boundary.decode_range())),
        )
    }

    #[cfg(test)]
    pub(super) fn record_finalized(
        &self,
        result: Result<String, TranscribeError>,
        samples: Option<u64>,
    ) {
        let mut state = Self::lock(&self.finalized);
        match result {
            Ok(text) if state.failure.is_none() => {
                append_transcript(&mut state.text, &text);
                if let Some(samples) = samples {
                    state.samples = samples;
                }
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
    ) {
        let mut state = Self::lock(&self.finalized);
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
            FinalBoundary::Natural => record_natural_outcome(&mut state, outcome, accepted),
            FinalBoundary::Forced(boundary) => {
                let FinalRangeResult::Decoded(text) = outcome.result else {
                    state.failure = Some(Arc::new(TakeFailure::ForcedWindowNotDecoded));
                    return;
                };
                record_forced_outcome(&mut state, boundary, text, accepted);
            }
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
            state
                .pending_forced
                .as_ref()
                .map(|pending| pending.boundary.decode_range()),
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
) {
    let flushed = flush_pending_forced(state, accepted);
    debug_assert!(flushed);
    match &outcome.result {
        FinalRangeResult::Decoded(text) => {
            settle_skipped(state, accepted, false);
            append_transcript(&mut state.text, text);
            state.samples = outcome.range.end;
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
        state.pending_forced = Some(PendingForced { boundary, text });
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
    let previous_range = previous.boundary.decode_range();
    let current_range = boundary.decode_range();
    let prefix_end = if let Some(prefix_end) = range_guided_suffix_prefix_start(
        &previous.text,
        previous_range.clone(),
        &text,
        current_range.clone(),
        overlap.clone(),
    ) {
        prefix_end
    } else {
        let Some(projection) =
            projected_prefix_end(&previous.text, previous_range.clone(), overlap.start)
        else {
            state.pending_forced = Some(previous);
            state.failure = Some(Arc::new(TakeFailure::ForcedOverlapInconsistent));
            return;
        };
        tracing::warn!(
            warning_code = "forced_final_overlap_estimated",
            prior_decode_start = previous_range.start,
            prior_decode_end = previous_range.end,
            current_decode_start = current_range.start,
            current_decode_end = current_range.end,
            overlap_start = overlap.start,
            overlap_end = overlap.end,
            projection_input_bytes = projection.metrics.input_bytes,
            projection_tokens = projection.metrics.tokens,
            projection_audio_before_overlap = projection.metrics.audio_before_overlap,
            projection_audio_total = projection.metrics.audio_total,
            projection_rounded_tokens = projection.metrics.rounded_tokens,
            projection_selected_tokens = projection.metrics.selected_tokens,
            projection_punctuation_examined = projection.metrics.punctuation_examined,
            projection_punctuation_candidates = projection.metrics.punctuation_candidates,
            projection_rounding = "nearest_ties_earlier",
            "estimated forced final overlap reconciliation"
        );
        projection.byte_end
    };
    settle_skipped(state, accepted, false);
    settle_decoded(
        state,
        previous.boundary.decode_range().start..overlap.start,
        previous.text[..prefix_end].trim_end(),
    );
    state.pending_forced = Some(PendingForced { boundary, text });
}

fn flush_pending_forced(state: &mut FinalizedState, accepted: &[AcceptedHypothesis]) -> bool {
    let Some(pending) = state.pending_forced.take() else {
        return true;
    };
    settle_skipped(state, accepted, false);
    settle_decoded(state, pending.boundary.decode_range(), &pending.text);
    true
}

fn settle_decoded(state: &mut FinalizedState, range: std::ops::Range<u64>, text: &str) {
    append_transcript(&mut state.text, text);
    state.samples = range.end;
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
        .filter(|candidate| candidate.range().end > state.samples)
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
