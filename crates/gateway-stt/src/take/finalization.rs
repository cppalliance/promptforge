use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gateway_stt_engine::{DecodeRequest, TranscribeError};
use tokio::sync::{mpsc, oneshot};

use crate::generation::GenerationLease;
use crate::segment::{ForcedBoundary, SegmentOutcome};

use super::final_decode::process_samples;
use super::final_outcome::{FinalRangeOutcome, SkipReason};
use super::state::TakeState;
use super::window::AcceptedHypothesis;

pub(super) type TakeFinalization = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
pub(super) const FINAL_SEGMENT_CAPACITY: usize = 4;

#[derive(Debug)]
pub(super) enum FinalCommand {
    Segment {
        range: Range<u64>,
        forced: Option<ForcedBoundary>,
        leading_silence: Option<Range<u64>>,
        owner: FinalSegmentOwner,
    },
    Skipped {
        range: Range<u64>,
        reason: SkipReason,
        leading_silence: Option<Range<u64>>,
        owner: FinalSegmentOwner,
    },
    Complete {
        committed_samples: u64,
        accepted: Vec<AcceptedHypothesis>,
        reply: oneshot::Sender<Result<String, String>>,
    },
}

#[derive(Debug)]
pub(super) struct FinalPipeline {
    commands: mpsc::Sender<FinalCommand>,
    task: tokio::task::JoinHandle<()>,
    pending_segments: Arc<AtomicUsize>,
}

impl Drop for FinalPipeline {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FinalPipeline {
    pub(super) fn submit_closed_segments(&self, state: &TakeState) {
        loop {
            let command = {
                let buffer = TakeState::lock(&state.buffer);
                let mut segmenter = TakeState::lock(&state.segmenter);
                let previous_consumed = segmenter.consumed();
                let Some(outcome) = segmenter.poll(buffer.samples(), buffer.origin()) else {
                    break;
                };
                let Some(owner) = FinalSegmentOwner::reserve(&self.pending_segments) else {
                    state.record_failure("final segment capacity is reached".to_owned());
                    break;
                };
                let range = match &outcome {
                    SegmentOutcome::Decode(range) | SegmentOutcome::Skipped(range) => range.clone(),
                    SegmentOutcome::Forced(boundary) => boundary.decode_range(),
                };
                let leading_silence =
                    (previous_consumed < range.start).then(|| previous_consumed..range.start);
                match outcome {
                    SegmentOutcome::Decode(range) => FinalCommand::Segment {
                        range,
                        forced: None,
                        leading_silence,
                        owner,
                    },
                    SegmentOutcome::Forced(boundary) => FinalCommand::Segment {
                        range,
                        forced: Some(boundary),
                        leading_silence,
                        owner,
                    },
                    SegmentOutcome::Skipped(range) => FinalCommand::Skipped {
                        range,
                        reason: SkipReason::BelowSpeechThreshold,
                        leading_silence,
                        owner,
                    },
                }
            };
            match self.commands.try_send(command) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    state.record_failure("final segment capacity is reached".to_owned());
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    state.record_failure("final transcription pipeline exited".to_owned());
                    break;
                }
            }
        }
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub(super) fn pending_segments(&self) -> usize {
        self.pending_segments.load(Ordering::Acquire)
    }

    pub(super) fn finalization(
        &self,
        committed_samples: u64,
        accepted: Vec<AcceptedHypothesis>,
    ) -> TakeFinalization {
        let commands = self.commands.clone();
        Box::pin(async move {
            let (reply, reply_rx) = oneshot::channel();
            if commands
                .send(FinalCommand::Complete {
                    committed_samples,
                    accepted,
                    reply,
                })
                .await
                .is_err()
            {
                return Err("final transcription pipeline exited".to_owned());
            }
            reply_rx
                .await
                .unwrap_or_else(|_| Err("final transcription pipeline exited".to_owned()))
        })
    }
}

#[derive(Debug)]
pub(super) struct FinalSegmentOwner {
    pending_segments: Arc<AtomicUsize>,
}

impl FinalSegmentOwner {
    pub(super) fn reserve(pending_segments: &Arc<AtomicUsize>) -> Option<Self> {
        pending_segments
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                (pending < FINAL_SEGMENT_CAPACITY).then_some(pending + 1)
            })
            .ok()?;
        Some(Self {
            pending_segments: Arc::clone(pending_segments),
        })
    }
}

impl Drop for FinalSegmentOwner {
    fn drop(&mut self) {
        self.pending_segments.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn spawn_final_pipeline(
    engine: GenerationLease,
    guidance: Arc<[String]>,
    state: Arc<TakeState>,
) -> FinalPipeline {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let pending_segments = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        guidance,
        state,
        move |request| {
            let engine = engine.clone();
            async move {
                if !engine.has_final_pass() {
                    return None;
                }
                Some(engine.decode(request).await)
            }
        },
    ));
    FinalPipeline {
        commands,
        task,
        pending_segments,
    }
}

pub(super) async fn run_final_pipeline<D, F>(
    mut receiver: mpsc::Receiver<FinalCommand>,
    guidance: Arc<[String]>,
    state: Arc<TakeState>,
    mut decode: D,
) where
    D: FnMut(DecodeRequest) -> F,
    F: Future<Output = Option<Result<String, TranscribeError>>>,
{
    while let Some(command) = receiver.recv().await {
        match command {
            FinalCommand::Segment {
                range,
                forced,
                leading_silence,
                owner,
            } => {
                record_leading_silence(&state, leading_silence);
                let samples = TakeState::lock(&state.buffer)
                    .transfer_range(range.clone())
                    .unwrap_or_else(|_| panic!("ordered segment range must remain resident"));
                process_samples(&state, &guidance, &mut decode, samples, range, forced).await;
                drop(owner);
            }
            FinalCommand::Skipped {
                range,
                reason,
                leading_silence,
                owner,
            } => {
                record_leading_silence(&state, leading_silence);
                TakeState::lock(&state.buffer)
                    .compact_to(range.end)
                    .unwrap_or_else(|_| panic!("ordered skipped range must remain resident"));
                state.record_final_outcome(FinalRangeOutcome::skipped(range, reason));
                drop(owner);
            }
            FinalCommand::Complete {
                committed_samples,
                accepted,
                reply,
            } => {
                let (start, forced) = {
                    let segmenter = TakeState::lock(&state.segmenter);
                    (
                        segmenter.consumed(),
                        segmenter.terminal_boundary(committed_samples),
                    )
                };
                let range = forced
                    .as_ref()
                    .map_or(start..committed_samples, ForcedBoundary::decode_range);
                let tail = TakeState::lock(&state.buffer)
                    .transfer_range(range.clone())
                    .unwrap_or_else(|_| panic!("ordered final tail must remain resident"));
                process_samples(&state, &guidance, &mut decode, tail, range, forced).await;
                drop(reply.send(state.completion(&accepted, committed_samples)));
                break;
            }
        }
    }
}

fn record_leading_silence(state: &TakeState, range: Option<Range<u64>>) {
    if let Some(range) = range {
        state.record_final_outcome(FinalRangeOutcome::skipped(range, SkipReason::Silence));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::{mpsc, oneshot};

    use super::{FINAL_SEGMENT_CAPACITY, FinalCommand, FinalPipeline, run_final_pipeline};
    use crate::take::Take;
    use crate::take::state::TakeState;
    use crate::take::window::AcceptedHypothesis;

    fn accepted_from_snapshot(
        take: &Take,
        range: std::ops::Range<u64>,
        text: &str,
        committed_samples: u64,
    ) -> Vec<AcceptedHypothesis> {
        take.next_window_snapshot(text, range.start, range.start, range.end)
            .expect("the production window snapshot is accepted");
        TakeState::lock(&take.whole_window).accepted_hypotheses(committed_samples)
    }

    #[tokio::test]
    async fn natural_handoff_stays_resident_until_ordered_pipeline_transfer() {
        let (commands, mut receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
        let pending = Arc::new(AtomicUsize::new(0));
        let pipeline = FinalPipeline {
            commands,
            task: tokio::spawn(std::future::pending()),
            pending_segments: Arc::clone(&pending),
        };
        let state = TakeState::default();
        let mut samples = vec![0.5; 16_000];
        samples.extend(vec![0.0; 48_000]);
        TakeState::lock(&state.buffer)
            .append(samples)
            .expect("resident PCM reserves");

        pipeline.submit_closed_segments(&state);

        let command = receiver.try_recv().expect("closed segment queues");
        let FinalCommand::Segment { range, owner, .. } = command else {
            panic!("ordinary speech queues a final decode");
        };
        let resident = TakeState::lock(&state.buffer);
        assert_eq!(resident.origin(), 0);
        assert!(resident.end() >= range.end);
        assert_eq!(pending.load(Ordering::Acquire), 1);
        drop(resident);
        drop(owner);
        assert_eq!(pending.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn saturated_handoff_does_not_compact_unowned_pcm() {
        let (commands, mut receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
        let pending = Arc::new(AtomicUsize::new(FINAL_SEGMENT_CAPACITY));
        let pipeline = FinalPipeline {
            commands,
            task: tokio::spawn(std::future::pending()),
            pending_segments: pending,
        };
        let state = TakeState::default();
        let mut samples = vec![0.5; 16_000];
        samples.extend(vec![0.0; 48_000]);
        TakeState::lock(&state.buffer)
            .append(samples)
            .expect("resident PCM reserves");

        pipeline.submit_closed_segments(&state);

        assert!(receiver.try_recv().is_err());
        assert_eq!(TakeState::lock(&state.buffer).origin(), 0);
        assert_eq!(
            state.pending_failure().as_deref(),
            Some("final segment capacity is reached")
        );
    }

    #[tokio::test]
    async fn pipeline_reconciles_short_tail_without_decoding_it() {
        let (commands, receiver) = mpsc::channel(1);
        let take = Take::without_final(Vec::new());
        take.append(vec![0.5; 4_800]).expect("tail PCM reserves");
        let accepted = accepted_from_snapshot(&take, 0..4_800, "last word", 4_800);
        let state = Arc::clone(&take.state);
        let calls = Arc::new(AtomicUsize::new(0));
        let decode_calls = Arc::clone(&calls);
        let task = tokio::spawn(run_final_pipeline(
            receiver,
            Arc::from([]),
            state,
            move |_| {
                decode_calls.fetch_add(1, Ordering::SeqCst);
                async { Some(Ok("must not decode".to_owned())) }
            },
        ));
        let (reply, completion) = oneshot::channel();
        commands
            .send(FinalCommand::Complete {
                committed_samples: 4_800,
                accepted,
                reply,
            })
            .await
            .expect("completion queues");

        assert_eq!(
            completion.await.expect("completion replies"),
            Ok("last word".to_owned())
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        task.await.expect("pipeline exits");
    }

    #[tokio::test]
    async fn pipeline_rejects_a_hypothesis_with_only_partial_skipped_coverage() {
        let (commands, receiver) = mpsc::channel(1);
        let take = Take::without_final(Vec::new());
        take.append(vec![0.5; 8_000]).expect("tail PCM reserves");
        let accepted = accepted_from_snapshot(&take, 0..8_000, "must not inherit", 8_000);
        TakeState::lock(&take.state.segmenter).set_consumed_for_test(4_000);
        let state = Arc::clone(&take.state);
        let calls = Arc::new(AtomicUsize::new(0));
        let decode_calls = Arc::clone(&calls);
        let task = tokio::spawn(run_final_pipeline(
            receiver,
            Arc::from([]),
            state,
            move |_| {
                decode_calls.fetch_add(1, Ordering::SeqCst);
                async { Some(Ok("must not decode".to_owned())) }
            },
        ));
        let (reply, completion) = oneshot::channel();
        commands
            .send(FinalCommand::Complete {
                committed_samples: 8_000,
                accepted,
                reply,
            })
            .await
            .expect("completion queues");

        assert_eq!(
            completion.await.expect("completion replies"),
            Ok(String::new())
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        task.await.expect("pipeline exits");
    }
}
