//! Per-take speech state and finalization ownership.

use std::sync::Arc;
use std::sync::Mutex;

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

#[cfg(test)]
use finalization::{FINAL_SEGMENT_CAPACITY, FinalCommand, FinalSegmentOwner, run_final_pipeline};
use finalization::{FinalPipeline, spawn_final_pipeline};
pub(crate) use interim::InterimSnapshot;
#[cfg(test)]
use pcm::PcmBudgetProbe;
use pcm::RetainedPcm;
use state::TakeState;
use window::WholeWindowState;

#[derive(Debug)]
pub(crate) struct InterimAudioWindow {
    pub(crate) samples: RetainedPcm,
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) segment_start: u64,
}

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
    pub(crate) fn new(guidance: Vec<String>, engine: Option<GenerationLease>) -> Self {
        Self::with_state(guidance, engine, TakeState::default())
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

    #[cfg(test)]
    fn without_final(guidance: Vec<String>) -> Self {
        Self::new(guidance, None)
    }

    pub(crate) fn guidance(&self) -> &[String] {
        &self.guidance
    }

    pub(crate) fn append(&self, samples: Vec<f32>) -> Result<(), AudioError> {
        TakeState::lock(&self.state.buffer).append(samples)
    }

    pub(crate) fn submit_closed_segments(&self) {
        if let Some(pipeline) = &self.final_pipeline {
            pipeline.submit_closed_segments(&self.state);
        }
    }

    pub(crate) fn consumed(&self) -> u64 {
        TakeState::lock(&self.state.segmenter).consumed()
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn uncommitted_snapshot(&self, window_samples: usize) -> Vec<f32> {
        let consumed = self.consumed();
        let buffer = TakeState::lock(&self.state.buffer);
        buffer.snapshot_from(consumed, window_samples)
    }

    pub(crate) fn interim_window(
        &self,
        window_samples: usize,
    ) -> Result<InterimAudioWindow, AudioError> {
        let segment_start = self.consumed();
        let buffer = TakeState::lock(&self.state.buffer);
        let end = buffer.end();
        let start = segment_start
            .max(end.saturating_sub(u64::try_from(window_samples).unwrap_or(u64::MAX)));
        Ok(InterimAudioWindow {
            samples: buffer.copy_range(start..end)?,
            start,
            end,
            segment_start,
        })
    }

    pub(crate) fn finalized(&self) -> String {
        self.state.finalized()
    }

    pub(crate) fn next_window_snapshot(
        &self,
        hypothesis: &str,
        segment_start: u64,
        window_start: u64,
        window_end: u64,
    ) -> Option<InterimSnapshot> {
        let live_prefix = self.state.live_prefix_snapshot();
        let update = TakeState::lock(&self.whole_window).try_next(
            &live_prefix,
            segment_start,
            window_start,
            window_end,
            hypothesis,
        );
        if let Ok(snapshot) = update {
            snapshot
        } else {
            self.state
                .record_failure("accepted hypothesis capacity is reached".to_owned());
            None
        }
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
    fn take_failure(&self) -> Option<String> {
        self.state.take_failure()
    }

    pub(crate) fn finalization(&self) -> Option<finalization::TakeFinalization> {
        let pipeline = self.final_pipeline.as_ref()?;
        let committed_samples = TakeState::lock(&self.state.buffer).end();
        let accepted = TakeState::lock(&self.whole_window).accepted_hypotheses(committed_samples);
        Some(pipeline.finalization(committed_samples, accepted))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, Weak};
    use std::time::Duration;

    use tokio::sync::{mpsc, oneshot};

    use super::{FinalCommand, FinalSegmentOwner, Take, run_final_pipeline};

    #[test]
    fn miri_final_segment_reservation_is_exact() {
        let pending = Arc::new(AtomicUsize::new(0));
        let mut owners = Vec::new();
        for _ in 0..super::FINAL_SEGMENT_CAPACITY {
            owners.push(
                FinalSegmentOwner::reserve(&pending).expect("capacity owns the admitted segment"),
            );
        }
        assert!(FinalSegmentOwner::reserve(&pending).is_none());
        assert_eq!(
            pending.load(Ordering::Acquire),
            super::FINAL_SEGMENT_CAPACITY
        );
        drop(owners);
        assert_eq!(pending.load(Ordering::Acquire), 0);
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
            Arc::new(Mutex::new(super::WholeWindowState::default())),
            move |_| {
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
