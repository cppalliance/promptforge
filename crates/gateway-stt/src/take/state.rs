use std::sync::{Mutex, MutexGuard, PoisonError};

#[cfg(test)]
use gateway_stt_engine::TranscribeError;

use super::final_outcome::{FinalRangeOutcome, FinalRangeResult, assemble_completion};
use super::pcm::{RetainedPcmBudget, RollingPcm};
use super::text::append_transcript;
use super::window::AcceptedHypothesis;
use crate::segment::Segmenter;

pub(super) const MAX_PENDING_FINAL_OUTCOMES: usize = 4_096;

#[derive(Debug, Default)]
struct FinalizedState {
    text: String,
    failure: Option<String>,
    samples: u64,
    outcomes: Vec<FinalRangeOutcome>,
    has_skipped_coverage: bool,
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
        if let FinalRangeResult::Decoded(text) = &outcome.result
            && !state.has_skipped_coverage
        {
            append_transcript(&mut state.text, text);
            state.samples = outcome.range.end;
            return;
        }
        if state.outcomes.len() == MAX_PENDING_FINAL_OUTCOMES {
            state.failure = Some("final outcome capacity is reached".to_owned());
            return;
        }
        match &outcome.result {
            FinalRangeResult::Decoded(_) => {}
            FinalRangeResult::Skipped(_) => state.has_skipped_coverage = true,
        }
        state.outcomes.push(outcome);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    use gateway_stt_engine::TranscribeError;

    use super::{MAX_PENDING_FINAL_OUTCOMES, TakeState};
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
}
