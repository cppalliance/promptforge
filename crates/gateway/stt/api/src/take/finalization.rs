//! Final-decode pipeline that sequences closed segments into a take completion.

use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gateway_stt_engine::{DecodeOutput, DecodeRequest, TranscribeError};
use tokio::sync::{mpsc, oneshot};

use crate::generation::GenerationLease;
use crate::segment::{ForcedBoundary, SegmentOutcome};

use super::final_decode::process_samples;
use super::final_outcome::{FinalRangeOutcome, SkipReason};
use super::state::{TakeFailure, TakeState};
use super::window::{AcceptedHypothesis, WholeWindowState};

pub(super) type TakeFinalization =
    Pin<Box<dyn Future<Output = Result<String, Arc<TakeFailure>>> + Send>>;
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
        reply: oneshot::Sender<Result<String, Arc<TakeFailure>>>,
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
                    state.record_failure(TakeFailure::SegmentCapacity);
                    break;
                };
                let range = match &outcome {
                    SegmentOutcome::Decode(range) | SegmentOutcome::Skipped(range) => range.clone(),
                    SegmentOutcome::Forced(boundary) => boundary.decode_range(),
                };
                let leading_silence =
                    (previous_consumed < range.start).then_some(previous_consumed..range.start);
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
                    state.record_failure(TakeFailure::SegmentCapacity);
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    state.record_failure(TakeFailure::PipelineExited);
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
                return Err(Arc::new(TakeFailure::PipelineExited));
            }
            reply_rx
                .await
                .unwrap_or_else(|_| Err(Arc::new(TakeFailure::PipelineExited)))
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
            .try_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
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
    whole_window: Arc<Mutex<WholeWindowState>>,
) -> FinalPipeline {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let pending_segments = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        guidance,
        state,
        whole_window,
        move |request| {
            let engine = engine.clone();
            async move {
                if !engine.has_final_pass() {
                    return None;
                }
                Some(engine.decode(request).await.map(DecodeOutput::into_text))
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
    whole_window: Arc<Mutex<WholeWindowState>>,
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
                record_leading_silence(&state, &whole_window, leading_silence);
                let samples = TakeState::lock(&state.buffer)
                    .transfer_range(range.clone())
                    .unwrap_or_else(|_| panic!("ordered segment range must remain resident"));
                process_samples(
                    &state,
                    &whole_window,
                    &guidance,
                    &mut decode,
                    samples,
                    range,
                    forced,
                )
                .await;
                drop(owner);
            }
            FinalCommand::Skipped {
                range,
                reason,
                leading_silence,
                owner,
            } => {
                record_leading_silence(&state, &whole_window, leading_silence);
                TakeState::lock(&state.buffer)
                    .compact_to(range.end)
                    .unwrap_or_else(|_| panic!("ordered skipped range must remain resident"));
                record_outcome(
                    &state,
                    &whole_window,
                    FinalRangeOutcome::skipped(range, reason),
                );
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
                process_samples(
                    &state,
                    &whole_window,
                    &guidance,
                    &mut decode,
                    tail,
                    range,
                    forced,
                )
                .await;
                drop(reply.send(state.completion(&accepted, committed_samples)));
                break;
            }
        }
    }
}

fn record_leading_silence(
    state: &TakeState,
    whole_window: &Mutex<WholeWindowState>,
    range: Option<Range<u64>>,
) {
    if let Some(range) = range {
        record_outcome(
            state,
            whole_window,
            FinalRangeOutcome::skipped(range, SkipReason::Silence),
        );
    }
}

pub(super) fn record_outcome(
    state: &TakeState,
    whole_window: &Mutex<WholeWindowState>,
    outcome: FinalRangeOutcome,
) {
    let window = TakeState::lock(whole_window);
    let accepted = window.accepted_hypotheses(outcome.range.end);
    let shown = window.shown();
    drop(window);
    state.record_final_outcome(outcome, &accepted, &shown);
}

#[cfg(test)]
#[path = "finalization-tests.rs"]
mod tests;
