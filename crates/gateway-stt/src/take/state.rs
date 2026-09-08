use std::sync::{Mutex, MutexGuard, PoisonError};

#[cfg(test)]
use gateway_stt_engine::TranscribeError;

use super::agreement::matching_suffix_prefix_start;
use super::final_outcome::{
    FinalBoundary, FinalRangeOutcome, FinalRangeResult, assemble_completion,
};
use super::pcm::{RetainedPcmBudget, RollingPcm};
use super::text::append_transcript;
use super::window::AcceptedHypothesis;
use crate::segment::{ForcedBoundary, Segmenter};

#[derive(Debug, Default)]
struct FinalizedState {
    text: String,
    failure: Option<String>,
    samples: u64,
    outcomes: Vec<FinalRangeOutcome>,
    pending_forced: Option<PendingForced>,
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

    pub(super) fn finalized_snapshot(&self) -> (String, u64) {
        self.finalized_snapshot_with(|| {})
    }

    fn finalized_snapshot_with(&self, synchronized: impl FnOnce()) -> (String, u64) {
        let state = Self::lock(&self.finalized);
        synchronized();
        (state.text.clone(), state.samples)
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
            Err(error) if state.failure.is_none() => state.failure = Some(error.to_string()),
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
        match outcome.boundary.clone() {
            FinalBoundary::Natural => record_natural_outcome(&mut state, outcome, accepted),
            FinalBoundary::Forced(boundary) => {
                let FinalRangeResult::Decoded(text) = outcome.result else {
                    state.failure = Some("forced final window was not decoded".to_owned());
                    return;
                };
                record_forced_outcome(&mut state, boundary, text, accepted);
            }
        }
    }

    pub(super) fn record_failure(&self, failure: String) {
        let mut state = Self::lock(&self.finalized);
        if state.failure.is_none() {
            state.failure = Some(failure);
        }
    }

    pub(super) fn has_failure(&self) -> bool {
        Self::lock(&self.finalized).failure.is_some()
    }

    pub(super) fn pending_failure(&self) -> Option<String> {
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
    pub(super) fn take_failure(&self) -> Option<String> {
        Self::lock(&self.finalized).failure.take()
    }

    pub(super) fn completion(
        &self,
        accepted: &[AcceptedHypothesis],
        committed_samples: u64,
    ) -> Result<String, String> {
        let mut state = Self::lock(&self.finalized);
        if state.failure.is_none() && !flush_pending_forced(&mut state, accepted) {
            state.failure = Some("final outcome capacity is reached".to_owned());
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
            state.failure = Some("forced final overlap metadata is inconsistent".to_owned());
            return;
        }
        state.pending_forced = Some(PendingForced { boundary, text });
        return;
    };
    let Some(previous) = state.pending_forced.take() else {
        state.failure = Some("forced final overlap metadata is inconsistent".to_owned());
        return;
    };
    if previous.boundary.decode_range().end != overlap.end
        || boundary.new_audio().start != overlap.end
    {
        state.pending_forced = Some(previous);
        state.failure = Some("forced final overlap metadata is inconsistent".to_owned());
        return;
    }
    let Some(prefix_end) = matching_suffix_prefix_start(&previous.text, &text) else {
        state.pending_forced = Some(previous);
        state.failure = Some("forced final overlap could not be aligned".to_owned());
        return;
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
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    use gateway_stt_engine::TranscribeError;

    use super::TakeState;
    use crate::segment::ForcedBoundary;
    use crate::take::final_outcome::{FinalRangeOutcome, SkipReason};
    use crate::take::window::AcceptedHypothesis;

    #[test]
    fn finalized_snapshot_cannot_mix_text_and_sample_ownership() {
        let state = Arc::new(TakeState::default());
        state.record_finalized(Ok::<_, TranscribeError>("old".to_owned()), Some(100));
        let writer_state = Arc::clone(&state);
        let (start, started) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            start.send(()).expect("snapshot knows the writer is ready");
            writer_state.record_finalized(Ok::<_, TranscribeError>("new".to_owned()), Some(200));
        });

        let snapshot = state.finalized_snapshot_with(|| {
            started
                .recv_timeout(Duration::from_secs(1))
                .expect("writer reaches the synchronized snapshot boundary");
            assert!(
                state.finalized.try_lock().is_err(),
                "the text and sample watermark share one held lock"
            );
        });
        writer.join().expect("finalization writer joins");

        assert_eq!(snapshot, ("old".to_owned(), 100));
        assert_eq!(state.finalized_snapshot(), ("old new".to_owned(), 200));
    }

    #[test]
    fn natural_speech_and_pause_cycles_settle_without_history_growth() {
        let state = TakeState::default();
        for index in 0..(4_096 * 2 + 17) {
            let start = u64::try_from(index).expect("test index fits") * 2;
            let decoded_end = start.saturating_add(1);
            state.record_final_outcome(
                FinalRangeOutcome::decoded(start..decoded_end, format!("word{index}")),
                &[],
            );
            state.record_final_outcome(
                FinalRangeOutcome::skipped(
                    decoded_end..decoded_end.saturating_add(1),
                    SkipReason::Silence,
                ),
                &[],
            );
            assert!(state.pending_failure().is_none());
            assert_eq!(state.coverage().2, 0);
        }
        assert_eq!(state.coverage().0, (4_096 * 2 + 17) as u64 * 2);
    }

    #[test]
    fn unresolved_skip_consumes_one_candidate_then_later_decode_settles_normally() {
        let state = TakeState::default();
        let accepted = [AcceptedHypothesis::new(0..2, "accepted".to_owned())];
        state.record_final_outcome(
            FinalRangeOutcome::skipped(0..1, SkipReason::BelowFinalWindow),
            &accepted,
        );
        assert_eq!(state.coverage(), (0, None, 1));

        state.record_final_outcome(
            FinalRangeOutcome::skipped(1..2, SkipReason::Silence),
            &accepted,
        );
        assert_eq!(state.coverage(), (2, None, 0));
        state.record_final_outcome(
            FinalRangeOutcome::decoded(2..3, "decoded".to_owned()),
            &accepted,
        );

        assert_eq!(
            state.finalized_snapshot(),
            ("accepted decoded".to_owned(), 3)
        );
        assert!(state.pending_failure().is_none());
    }

    #[test]
    fn forced_overlap_freezes_only_the_reconciled_old_prefix() {
        let state = TakeState::default();
        state.record_final_outcome(
            FinalRangeOutcome::forced(
                ForcedBoundary::first(0..160_000),
                "alpha beta ECHO, now".to_owned(),
            ),
            &[],
        );
        assert_eq!(state.finalized_snapshot(), (String::new(), 0));

        state.record_final_outcome(
            FinalRangeOutcome::forced(
                ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
                "echo now revised ending".to_owned(),
            ),
            &[],
        );
        assert_eq!(
            state.finalized_snapshot(),
            ("alpha beta".to_owned(), 32_000)
        );

        state.record_final_outcome(
            FinalRangeOutcome::decoded(320_000..336_000, "tail".to_owned()),
            &[],
        );
        assert_eq!(
            state.finalized_snapshot(),
            (
                "alpha beta echo now revised ending tail".to_owned(),
                336_000
            )
        );
    }

    #[test]
    fn forced_overlap_preserves_repeated_phrases_at_distinct_ranges() {
        let state = TakeState::default();
        state.record_final_outcome(
            FinalRangeOutcome::forced(
                ForcedBoundary::first(0..160_000),
                "echo now echo now".to_owned(),
            ),
            &[],
        );
        state.record_final_outcome(
            FinalRangeOutcome::forced(
                ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
                "echo now corrected".to_owned(),
            ),
            &[],
        );
        state.record_final_outcome(
            FinalRangeOutcome::skipped(320_000..320_000, SkipReason::BelowFinalWindow),
            &[],
        );

        assert_eq!(state.finalized(), "echo now echo now corrected");
        assert!(state.pending_failure().is_none());
    }

    #[test]
    fn missing_forced_overlap_fails_without_changing_canonical_text() {
        let state = TakeState::default();
        state.record_final_outcome(
            FinalRangeOutcome::decoded(0..16_000, "canonical".to_owned()),
            &[],
        );
        state.record_final_outcome(
            FinalRangeOutcome::forced(
                ForcedBoundary::first(16_000..176_000),
                "old overlap".to_owned(),
            ),
            &[],
        );
        let before = state.finalized_snapshot();

        state.record_final_outcome(
            FinalRangeOutcome::forced(
                ForcedBoundary::overlapping(48_000..176_000, 176_000..336_000),
                "unrelated revision".to_owned(),
            ),
            &[],
        );

        assert_eq!(state.finalized_snapshot(), before);
        assert_eq!(
            state.pending_failure().as_deref(),
            Some("forced final overlap could not be aligned")
        );
    }
}
