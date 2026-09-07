//! Per-take speech state and finalization ownership.

use std::sync::Arc;
use std::sync::Mutex;

#[cfg(test)]
use gateway_stt_engine::TranscribeError;

use crate::generation::GenerationLease;

mod agreement;
mod final_outcome;
mod finalization;
mod interim;
mod state;
mod text;
mod window;

#[cfg(test)]
use finalization::{FINAL_SEGMENT_CAPACITY, FinalCommand, reserve_segment, run_final_pipeline};
use finalization::{FinalPipeline, spawn_final_pipeline};
pub(crate) use interim::InterimSnapshot;
use state::TakeState;
use window::WholeWindowState;

fn tail(buffer: &[f32], window: usize) -> &[f32] {
    &buffer[buffer.len().saturating_sub(window)..]
}

#[derive(Debug)]
pub(crate) struct InterimAudioWindow {
    pub(crate) samples: Vec<f32>,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) segment_start: usize,
}

/// All mutable and immutable state belonging to one speech take.
#[derive(Debug)]
pub(crate) struct Take {
    guidance: Arc<[String]>,
    state: Arc<TakeState>,
    whole_window: Mutex<WholeWindowState>,
    final_pipeline: Option<FinalPipeline>,
}

impl Take {
    pub(crate) fn new(guidance: Vec<String>, engine: Option<GenerationLease>) -> Self {
        let guidance = Arc::<[String]>::from(guidance);
        let state = Arc::new(TakeState::default());
        let final_pipeline = engine
            .filter(GenerationLease::has_final_pass)
            .map(|engine| spawn_final_pipeline(engine, Arc::clone(&guidance), Arc::clone(&state)));
        Self {
            guidance,
            state,
            whole_window: Mutex::new(WholeWindowState::default()),
            final_pipeline,
        }
    }

    #[cfg(test)]
    fn without_final(guidance: Vec<String>) -> Self {
        Self::new(guidance, None)
    }

    pub(crate) fn guidance(&self) -> &[String] {
        &self.guidance
    }

    pub(crate) fn append(&self, samples: &[f32]) {
        TakeState::lock(&self.state.buffer).extend_from_slice(samples);
    }

    pub(crate) fn submit_closed_segments(&self) {
        if let Some(pipeline) = &self.final_pipeline {
            pipeline.submit_closed_segments(&self.state);
        }
    }

    pub(crate) fn consumed(&self) -> usize {
        TakeState::lock(&self.state.segmenter).consumed()
    }

    pub(crate) fn uncommitted_snapshot(&self, window_samples: usize) -> Vec<f32> {
        let consumed = self.consumed();
        let buffer = TakeState::lock(&self.state.buffer);
        let uncommitted = &buffer[consumed.min(buffer.len())..];
        tail(uncommitted, window_samples).to_vec()
    }

    pub(crate) fn interim_window(&self, window_samples: usize) -> InterimAudioWindow {
        let segment_start = self.consumed();
        let buffer = TakeState::lock(&self.state.buffer);
        let end = buffer.len();
        let start = segment_start.max(end.saturating_sub(window_samples));
        InterimAudioWindow {
            samples: buffer[start.min(end)..].to_vec(),
            start,
            end,
            segment_start,
        }
    }

    pub(crate) fn finalized(&self) -> String {
        self.state.finalized()
    }

    pub(crate) fn next_window_snapshot(
        &self,
        hypothesis: &str,
        segment_start: usize,
        window_start: usize,
        window_end: usize,
    ) -> Option<InterimSnapshot> {
        let (finalized, finalized_samples) = self.state.finalized_snapshot();
        TakeState::lock(&self.whole_window).next(
            &finalized,
            finalized_samples,
            segment_start,
            window_start,
            window_end,
            hypothesis,
        )
    }

    #[cfg(test)]
    fn record_finalized(&self, result: Result<String, TranscribeError>) {
        self.state.record_finalized(result, None);
    }

    pub(crate) fn record_failure(&self, failure: impl Into<String>) {
        self.state.record_failure(failure.into());
    }

    pub(crate) fn pending_failure(&self) -> Option<String> {
        self.state.pending_failure()
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn pending_final_segments(&self) -> usize {
        self.final_pipeline
            .as_ref()
            .map_or(0, FinalPipeline::pending_segments)
    }

    #[cfg(test)]
    fn take_failure(&self) -> Option<String> {
        self.state.take_failure()
    }

    pub(crate) fn finalization(&self) -> Option<finalization::TakeFinalization> {
        let pipeline = self.final_pipeline.as_ref()?;
        let consumed = self.consumed();
        let buffer = TakeState::lock(&self.state.buffer);
        let committed_samples = buffer.len();
        let tail = buffer[consumed.min(buffer.len())..].to_vec();
        drop(buffer);
        let accepted = TakeState::lock(&self.whole_window).accepted_hypotheses(committed_samples);
        Some(pipeline.finalization(tail, consumed, committed_samples, accepted))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Weak};
    use std::time::Duration;

    use tokio::sync::{mpsc, oneshot};

    use super::{FinalCommand, Take, reserve_segment, run_final_pipeline};

    #[test]
    fn miri_final_segment_reservation_is_exact() {
        let pending = AtomicUsize::new(0);
        for _ in 0..super::FINAL_SEGMENT_CAPACITY {
            assert!(reserve_segment(&pending));
        }
        assert!(!reserve_segment(&pending));
        assert_eq!(
            pending.load(Ordering::Acquire),
            super::FINAL_SEGMENT_CAPACITY
        );
    }

    #[test]
    fn tail_returns_the_trailing_window() {
        let buffer: Vec<f32> = (0u8..10).map(f32::from).collect();
        assert_eq!(super::tail(&buffer, 4), &[6.0, 7.0, 8.0, 9.0]);
        assert_eq!(super::tail(&buffer, 100), &buffer);
        assert_eq!(super::tail(&[], 4), &[] as &[f32]);
    }

    #[test]
    fn finalized_history_and_guidance_are_isolated_per_take() {
        let first = Take::without_final(vec!["MCP".to_owned()]);
        let second = Take::without_final(vec!["GGUF".to_owned()]);

        first.record_finalized(Ok("ask not".to_owned()));
        second.record_finalized(Ok("what you".to_owned()));

        assert_eq!(first.guidance(), ["MCP"]);
        assert_eq!(first.finalized(), "ask not");
        assert_eq!(second.guidance(), ["GGUF"]);
        assert_eq!(second.finalized(), "what you");
    }

    #[test]
    fn finalized_segments_aggregate_in_arrival_order() {
        let take = Take::without_final(Vec::new());
        take.record_finalized(Ok("ask not".to_owned()));
        take.record_finalized(Ok("what you can do".to_owned()));
        assert_eq!(take.finalized(), "ask not what you can do");
    }

    #[test]
    fn a_take_retains_its_first_final_failure() {
        let take = Take::without_final(Vec::new());
        take.record_failure("first");
        take.record_failure("second");
        let failure = take.take_failure().expect("the take owns its failure");
        assert_eq!(failure, "first");
    }

    #[tokio::test]
    async fn completed_pipeline_releases_its_retained_dependency() {
        let (commands, receiver) = mpsc::channel(super::FINAL_SEGMENT_CAPACITY);
        let state = Arc::new(super::TakeState::default());
        let retained = Arc::new(());
        let weak: Weak<()> = Arc::downgrade(&retained);
        let pipeline_retained = Arc::clone(&retained);
        let task = tokio::spawn(run_final_pipeline(
            receiver,
            Arc::from([]),
            state,
            Arc::new(AtomicUsize::new(0)),
            move |_, _, _| {
                let retained = Arc::clone(&pipeline_retained);
                async move {
                    drop(retained);
                    Some(Ok(String::new()))
                }
            },
        ));
        drop(retained);
        let (reply, completion) = oneshot::channel();
        commands
            .send(FinalCommand::Complete {
                tail: Vec::new(),
                start: 0,
                committed_samples: 0,
                accepted: Vec::new(),
                reply,
            })
            .await
            .expect("completion queues");

        assert_eq!(
            completion.await.expect("the completion pipeline replies"),
            Ok(String::new())
        );
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the completed pipeline terminates before the deadline")
            .expect("the completed pipeline task succeeds");
        assert!(
            weak.upgrade().is_none(),
            "pipeline completion releases its retained engine-like dependency"
        );
    }
}
