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

pub(super) const MAX_PENDING_FINAL_OUTCOMES: usize = 4_096;

#[derive(Debug, Default)]
struct FinalizedState {
    text: String,
    failure: Option<String>,
    samples: u64,
    outcomes: Vec<FinalRangeOutcome>,
    has_skipped_coverage: bool,
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

    pub(super) fn record_final_outcome(&self, outcome: FinalRangeOutcome) {
        let mut state = Self::lock(&self.finalized);
        if state.failure.is_some() {
            return;
        }
        match outcome.boundary.clone() {
            FinalBoundary::Natural => record_natural_outcome(&mut state, outcome),
            FinalBoundary::Forced(boundary) => {
                let FinalRangeResult::Decoded(text) = outcome.result else {
                    state.failure = Some("forced final window was not decoded".to_owned());
                    return;
                };
                record_forced_outcome(&mut state, boundary, text);
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
        if state.failure.is_none() && !flush_pending_forced(&mut state) {
            state.failure = Some("final outcome capacity is reached".to_owned());
        }
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

fn record_natural_outcome(state: &mut FinalizedState, outcome: FinalRangeOutcome) {
    let pending_slots = usize::from(state.pending_forced.is_some() && state.has_skipped_coverage);
    let outcome_slots = usize::from(
        matches!(outcome.result, FinalRangeResult::Skipped(_)) || state.has_skipped_coverage,
    );
    if state.outcomes.len() + pending_slots + outcome_slots > MAX_PENDING_FINAL_OUTCOMES {
        state.failure = Some("final outcome capacity is reached".to_owned());
        return;
    }
    let flushed = flush_pending_forced(state);
    debug_assert!(flushed);
    match &outcome.result {
        FinalRangeResult::Decoded(text) if !state.has_skipped_coverage => {
            append_transcript(&mut state.text, text);
            state.samples = outcome.range.end;
        }
        FinalRangeResult::Decoded(_) => state.outcomes.push(outcome),
        FinalRangeResult::Skipped(_) => {
            state.has_skipped_coverage = true;
            state.outcomes.push(outcome);
        }
    }
}

fn record_forced_outcome(state: &mut FinalizedState, boundary: ForcedBoundary, text: String) {
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
    if state.has_skipped_coverage && state.outcomes.len() == MAX_PENDING_FINAL_OUTCOMES {
        state.pending_forced = Some(previous);
        state.failure = Some("final outcome capacity is reached".to_owned());
        return;
    }
    settle_decoded(
        state,
        previous.boundary.decode_range().start..overlap.start,
        previous.text[..prefix_end].trim_end(),
    );
    state.pending_forced = Some(PendingForced { boundary, text });
}

fn flush_pending_forced(state: &mut FinalizedState) -> bool {
    let Some(pending) = state.pending_forced.take() else {
        return true;
    };
    if state.has_skipped_coverage && state.outcomes.len() == MAX_PENDING_FINAL_OUTCOMES {
        state.pending_forced = Some(pending);
        return false;
    }
    settle_decoded(state, pending.boundary.decode_range(), &pending.text);
    true
}

fn settle_decoded(state: &mut FinalizedState, range: std::ops::Range<u64>, text: &str) {
    if state.has_skipped_coverage {
        state
            .outcomes
            .push(FinalRangeOutcome::decoded(range, text.to_owned()));
    } else {
        append_transcript(&mut state.text, text);
        state.samples = range.end;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    use gateway_stt_engine::TranscribeError;

    use super::{MAX_PENDING_FINAL_OUTCOMES, TakeState};
    use crate::segment::ForcedBoundary;
    use crate::take::final_outcome::{FinalRangeOutcome, SkipReason};

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
    fn pending_final_outcome_history_has_an_exact_failure_bound() {
        let state = TakeState::default();
        for index in 0..MAX_PENDING_FINAL_OUTCOMES {
            let start = u64::try_from(index).expect("test index fits");
            let end = start + 1;
            state.record_final_outcome(FinalRangeOutcome::skipped(start..end, SkipReason::Silence));
        }
        assert!(state.pending_failure().is_none());

        let start = MAX_PENDING_FINAL_OUTCOMES as u64;
        let end = start + 1;
        state.record_final_outcome(FinalRangeOutcome::skipped(start..end, SkipReason::Silence));
        assert_eq!(
            state.pending_failure().as_deref(),
            Some("final outcome capacity is reached")
        );
    }

    #[test]
    fn forced_overlap_freezes_only_the_reconciled_old_prefix() {
        let state = TakeState::default();
        state.record_final_outcome(FinalRangeOutcome::forced(
            ForcedBoundary::first(0..160_000),
            "alpha beta ECHO, now".to_owned(),
        ));
        assert_eq!(state.finalized_snapshot(), (String::new(), 0));

        state.record_final_outcome(FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
            "echo now revised ending".to_owned(),
        ));
        assert_eq!(
            state.finalized_snapshot(),
            ("alpha beta".to_owned(), 32_000)
        );

        state.record_final_outcome(FinalRangeOutcome::decoded(
            320_000..336_000,
            "tail".to_owned(),
        ));
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
        state.record_final_outcome(FinalRangeOutcome::forced(
            ForcedBoundary::first(0..160_000),
            "echo now echo now".to_owned(),
        ));
        state.record_final_outcome(FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
            "echo now corrected".to_owned(),
        ));
        state.record_final_outcome(FinalRangeOutcome::skipped(
            320_000..320_000,
            SkipReason::BelowFinalWindow,
        ));

        assert_eq!(state.finalized(), "echo now echo now corrected");
        assert!(state.pending_failure().is_none());
    }

    #[test]
    fn missing_forced_overlap_fails_without_changing_canonical_text() {
        let state = TakeState::default();
        state.record_final_outcome(FinalRangeOutcome::decoded(
            0..16_000,
            "canonical".to_owned(),
        ));
        state.record_final_outcome(FinalRangeOutcome::forced(
            ForcedBoundary::first(16_000..176_000),
            "old overlap".to_owned(),
        ));
        let before = state.finalized_snapshot();

        state.record_final_outcome(FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(48_000..176_000, 176_000..336_000),
            "unrelated revision".to_owned(),
        ));

        assert_eq!(state.finalized_snapshot(), before);
        assert_eq!(
            state.pending_failure().as_deref(),
            Some("forced final overlap could not be aligned")
        );
    }
}
