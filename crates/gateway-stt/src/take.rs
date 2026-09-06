//! Per-take speech state and finalization ownership.

use std::sync::Arc;

#[cfg(test)]
use gateway_stt_engine::TranscribeError;

use crate::generation::GenerationLease;

mod agreement;
mod finalization;
mod state;
mod text;

#[cfg(test)]
use agreement::LocalAgreement;
#[cfg(test)]
use finalization::{FINAL_SEGMENT_CAPACITY, FinalCommand, reserve_segment, run_final_pipeline};
use finalization::{FinalPipeline, spawn_final_pipeline};
use state::TakeState;
use text::append_transcript;

fn tail(buffer: &[f32], window: usize) -> &[f32] {
    &buffer[buffer.len().saturating_sub(window)..]
}

/// All mutable and immutable state belonging to one speech take.
#[derive(Debug)]
pub(crate) struct Take {
    guidance: Arc<[String]>,
    state: Arc<TakeState>,
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

    pub(crate) fn fallback_snapshot(&self, window_samples: usize) -> Vec<f32> {
        let finalized = self.state.finalized_samples();
        let buffer = TakeState::lock(&self.state.buffer);
        let pending = &buffer[finalized.min(buffer.len())..];
        tail(pending, window_samples).to_vec()
    }

    pub(crate) fn fallback_len(&self) -> usize {
        let finalized = self.state.finalized_samples();
        TakeState::lock(&self.state.buffer)
            .len()
            .saturating_sub(finalized)
    }

    pub(crate) fn finalized(&self) -> String {
        self.state.finalized()
    }

    pub(crate) fn fallback_transcript(&self, tail: &str) -> String {
        let mut transcript = self.finalized();
        append_transcript(&mut transcript, tail);
        transcript
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

    pub(crate) fn next_interim(&self, hypothesis: &str) -> Option<(String, String)> {
        let finalized = self.finalized();
        TakeState::lock(&self.state.interim).next(&finalized, hypothesis)
    }

    pub(crate) fn finalization(&self) -> Option<finalization::TakeFinalization> {
        let pipeline = self.final_pipeline.as_ref()?;
        let consumed = self.consumed();
        let buffer = TakeState::lock(&self.state.buffer);
        let tail = buffer[consumed.min(buffer.len())..].to_vec();
        Some(pipeline.finalization(tail))
    }

    pub(crate) async fn complete(&self) -> Option<Result<String, String>> {
        Some(self.finalization()?.await)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Weak};
    use std::time::Duration;

    use tokio::sync::{mpsc, oneshot};

    use super::{FinalCommand, LocalAgreement, Take, reserve_segment, run_final_pipeline};

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
    fn local_agreement_requires_two_hypotheses_and_preserves_whitespace() {
        let mut agreement = LocalAgreement::default();
        let first = agreement.observe("ask not what");
        assert_eq!(first.agreed, "");
        assert_eq!(first.tentative, "ask not what");

        let second = agreement.observe("ask not who");
        assert_eq!(second.agreed, "ask not");
        assert_eq!(second.tentative, " who");
        assert_eq!(
            format!("{}{}", second.agreed, second.tentative),
            "ask not who"
        );
    }

    #[test]
    fn production_interims_promote_locally_agreed_words() {
        let take = Take::without_final(Vec::new());
        assert_eq!(
            take.next_interim("ask not what"),
            Some((String::new(), "ask not what".to_owned()))
        );
        assert_eq!(
            take.next_interim("ask not who"),
            Some(("ask not".to_owned(), "who".to_owned()))
        );
        assert_eq!(
            take.next_interim("ask not who"),
            Some(("ask not who".to_owned(), String::new()))
        );
        assert_eq!(
            take.next_interim("ask not when"),
            Some(("ask not who".to_owned(), "when".to_owned()))
        );
    }

    #[test]
    fn finalization_preserves_a_divergent_promoted_prefix() {
        let take = Take::without_final(Vec::new());
        assert_eq!(
            take.next_interim("ask not your country"),
            Some((String::new(), "ask not your country".to_owned()))
        );
        let promoted = take
            .next_interim("ask not your country")
            .expect("the repeated hypothesis promotes its words")
            .0;
        assert_eq!(promoted, "ask not your country");

        take.record_finalized(Ok("ask not your kingdom".to_owned()));
        let committed = take
            .next_interim("new tail")
            .expect("speech after finalization emits another interim")
            .0;

        assert_eq!(committed, promoted);
        assert!(committed.starts_with("ask not your country"));
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

    #[test]
    fn tail_failure_fallback_preserves_successful_closed_segments() {
        let take = Take::without_final(Vec::new());
        take.record_finalized(Ok("successful segment".to_owned()));
        take.record_failure("tail failed");

        assert_eq!(take.state.completion(), Err("tail failed".to_owned()));
        assert_eq!(
            take.fallback_transcript("fallback tail"),
            "successful segment fallback tail"
        );
    }

    #[test]
    fn closed_segment_failure_does_not_duplicate_a_successful_tail_in_fallback() {
        let take = Take::without_final(Vec::new());
        take.record_failure("closed segment failed");
        take.record_finalized(Ok("successful tail".to_owned()));

        assert_eq!(
            take.state.completion(),
            Err("closed segment failed".to_owned())
        );
        assert_eq!(take.fallback_transcript("fallback tail"), "fallback tail");
    }

    #[tokio::test]
    async fn failed_segment_audio_remains_in_the_fallback_window() {
        let take = Take::without_final(Vec::new());
        let successful = vec![1.0; 4];
        let failed = vec![2.0; 3];
        let skipped = vec![3.0; 2];
        let tail = vec![4.0];
        take.append(
            &[
                successful.clone(),
                failed.clone(),
                skipped.clone(),
                tail.clone(),
            ]
            .concat(),
        );

        let (commands, receiver) = mpsc::channel(super::FINAL_SEGMENT_CAPACITY);
        let calls = Arc::new(AtomicUsize::new(0));
        let decode_calls = Arc::clone(&calls);
        let task = tokio::spawn(run_final_pipeline(
            receiver,
            Arc::from([]),
            Arc::clone(&take.state),
            Arc::new(AtomicUsize::new(3)),
            move |_, _, _| {
                let call = decode_calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    Some(match call {
                        0 => Ok("successful".to_owned()),
                        1 => return None,
                        _ => panic!("decoding must stop after the first failure"),
                    })
                }
            },
        ));
        commands
            .send(FinalCommand::Segment {
                samples: successful,
                end: 4,
            })
            .await
            .expect("the successful segment queues");
        commands
            .send(FinalCommand::Segment {
                samples: failed.clone(),
                end: 7,
            })
            .await
            .expect("the failed segment queues");
        commands
            .send(FinalCommand::Segment {
                samples: skipped.clone(),
                end: 9,
            })
            .await
            .expect("the skipped segment queues");
        let (reply, completion) = oneshot::channel();
        commands
            .send(FinalCommand::Complete {
                tail: tail.clone(),
                reply,
            })
            .await
            .expect("completion queues");

        assert!(
            completion
                .await
                .expect("the completion pipeline replies")
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the completed pipeline terminates before the deadline")
            .expect("the completed pipeline task succeeds");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            take.fallback_snapshot(usize::MAX),
            [failed, skipped, tail].concat()
        );
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
