//! Scripted speech detector driven by absolute sample runs.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{DetectorError, SpeechDetector};

#[derive(Debug, Default)]
struct DetectorState {
    runs: Vec<(usize, usize)>,
    failure_chunk: Option<usize>,
    chunk_starts: Vec<usize>,
    next_start: usize,
}

/// A cloneable [`SpeechDetector`] that reads speech from absolute sample runs.
///
/// Chunks are counted from sample 0, each starting where the previous one
/// ended. A chunk is speech when it overlaps any half-open `(start, end)` run.
/// Clones share one script and one record of the chunk starts seen.
#[derive(Clone, Debug, Default)]
pub struct ScriptedDetector {
    shared: Arc<Mutex<DetectorState>>,
}

impl ScriptedDetector {
    /// Creates a detector whose speech is the supplied half-open
    /// `(start, end)` sample runs.
    #[must_use]
    pub fn new(runs: impl IntoIterator<Item = (usize, usize)>) -> Self {
        let state = DetectorState {
            runs: runs.into_iter().collect(),
            ..DetectorState::default()
        };
        Self {
            shared: Arc::new(Mutex::new(state)),
        }
    }

    /// Makes the zero-based `chunk` fail with an inference error.
    #[must_use]
    pub fn with_failure_at(self, chunk: usize) -> Self {
        self.state().failure_chunk = Some(chunk);
        self
    }

    /// Returns the start sample of every chunk seen, including a failed one.
    #[must_use]
    pub fn chunk_starts(&self) -> Vec<usize> {
        self.state().chunk_starts.clone()
    }

    fn state(&self) -> MutexGuard<'_, DetectorState> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SpeechDetector for ScriptedDetector {
    fn classify(&mut self, chunk: &[f32]) -> Result<bool, DetectorError> {
        let mut state = self.state();
        let start = state.next_start;
        let end = start + chunk.len();
        let index = state.chunk_starts.len();
        state.chunk_starts.push(start);
        state.next_start = end;
        if state.failure_chunk == Some(index) {
            return Err(DetectorError::inference(format!(
                "scripted detector failure at chunk {index}"
            )));
        }
        Ok(state
            .runs
            .iter()
            .any(|&(run_start, run_end)| run_start.max(start) < run_end.min(end)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EnginePolicy;

    const CHUNK: usize = EnginePolicy::DETECTOR_CHUNK_SAMPLES;

    fn classify_chunks(detector: &mut dyn SpeechDetector, count: usize) -> Vec<Option<bool>> {
        let chunk = [0.0; CHUNK];
        (0..count).map(|_| detector.classify(&chunk).ok()).collect()
    }

    #[test]
    fn miri_scripted_detector_reads_speech_only_in_chunks_overlapping_a_run() {
        let observer = ScriptedDetector::new([(CHUNK - 1, CHUNK), (3 * CHUNK - 1, 3 * CHUNK + 1)]);
        let mut detector: Box<dyn SpeechDetector> = Box::new(observer.clone());

        assert_eq!(
            classify_chunks(detector.as_mut(), 5),
            [Some(true), Some(false), Some(true), Some(true), Some(false)]
        );
        assert_eq!(
            observer.chunk_starts(),
            [0, CHUNK, 2 * CHUNK, 3 * CHUNK, 4 * CHUNK]
        );
    }

    #[test]
    fn miri_scripted_detector_starts_each_chunk_where_the_last_one_ended() {
        let mut detector = ScriptedDetector::new([(100, 101)]);

        assert_eq!(detector.classify(&[0.0; 100]).ok(), Some(false));
        assert_eq!(detector.classify(&[0.0; CHUNK]).ok(), Some(true));
        assert_eq!(detector.chunk_starts(), [0, 100]);
    }

    #[test]
    fn miri_scripted_detector_ignores_empty_runs() {
        let mut detector = ScriptedDetector::new([(10, 10)]);

        assert_eq!(classify_chunks(&mut detector, 1), [Some(false)]);
    }

    #[test]
    fn miri_scripted_detector_fails_only_at_the_requested_chunk() {
        let mut detector = ScriptedDetector::new([(0, 4 * CHUNK)]).with_failure_at(1);
        let chunk = [0.0; CHUNK];

        assert_eq!(detector.classify(&chunk).ok(), Some(true));
        let error = detector.classify(&chunk).expect_err("chunk 1 fails");
        assert!(matches!(error, DetectorError::Inference(_)));
        assert_eq!(detector.classify(&chunk).ok(), Some(true));
        assert_eq!(detector.chunk_starts(), [0, CHUNK, 2 * CHUNK]);
    }
}
