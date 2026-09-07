use std::sync::{Mutex, MutexGuard, PoisonError};

use gateway_stt_engine::TranscribeError;

use super::interim::InterimState;
use super::text::append_transcript;
use crate::segment::Segmenter;

#[derive(Debug, Default)]
struct FinalizedState {
    text: String,
    failure: Option<String>,
    samples: usize,
}

#[derive(Debug, Default)]
pub(super) struct TakeState {
    pub(super) buffer: Mutex<Vec<f32>>,
    pub(super) segmenter: Mutex<Segmenter>,
    finalized: Mutex<FinalizedState>,
    pub(super) interim: Mutex<InterimState>,
}

impl TakeState {
    pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn finalized(&self) -> String {
        Self::lock(&self.finalized).text.clone()
    }

    pub(super) fn finalized_snapshot(&self) -> (String, usize) {
        self.finalized_snapshot_with(|| {})
    }

    fn finalized_snapshot_with(&self, synchronized: impl FnOnce()) -> (String, usize) {
        let state = Self::lock(&self.finalized);
        synchronized();
        (state.text.clone(), state.samples)
    }

    pub(super) fn record_finalized(
        &self,
        result: Result<String, TranscribeError>,
        samples: Option<usize>,
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

    pub(super) fn record_failure(&self, failure: String) {
        let mut state = Self::lock(&self.finalized);
        if state.failure.is_none() {
            state.failure = Some(failure);
        }
    }

    pub(super) fn has_failure(&self) -> bool {
        Self::lock(&self.finalized).failure.is_some()
    }

    pub(super) fn finalized_samples(&self) -> usize {
        Self::lock(&self.finalized).samples
    }

    pub(super) fn pending_failure(&self) -> Option<String> {
        Self::lock(&self.finalized).failure.clone()
    }

    #[cfg(test)]
    pub(super) fn take_failure(&self) -> Option<String> {
        Self::lock(&self.finalized).failure.take()
    }

    pub(super) fn completion(&self) -> Result<String, String> {
        let mut state = Self::lock(&self.finalized);
        match state.failure.take() {
            Some(failure) => Err(failure),
            None => Ok(state.text.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    use gateway_stt_engine::TranscribeError;

    use super::TakeState;

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
}
