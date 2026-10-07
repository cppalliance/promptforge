//! Final-decode pipeline that sequences closed segments into a take completion.

use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gateway_stt_engine::{DecodeOutput, DecodeRequest, TranscribeError};
use tokio::sync::{mpsc, oneshot};

use crate::generation::GenerationLease;
use crate::segment::ForcedBoundary;

use super::final_decode::process_samples;
use super::final_outcome::{FinalRangeOutcome, SkipReason};
use super::state::{TakeFailure, TakeState};
use super::window::{AcceptedHypothesis, WholeWindowState};

mod retry;

pub(super) use retry::{ClosedRange, append_releasing};

pub(super) type TakeFinalization =
    Pin<Box<dyn Future<Output = Result<String, Arc<TakeFailure>>> + Send>>;
pub(super) const FINAL_SEGMENT_CAPACITY: usize = 4;

#[derive(Debug)]
pub(super) enum FinalCommand {
    Closed {
        closed: ClosedRange,
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
            FinalCommand::Closed { closed, owner } => {
                process_closed(&state, &whole_window, &guidance, &mut decode, closed).await;
                drop(owner);
            }
            FinalCommand::Complete {
                committed_samples,
                accepted,
                reply,
            } => {
                let held = std::mem::take(&mut *TakeState::lock(&state.held));
                for closed in held {
                    process_closed(&state, &whole_window, &guidance, &mut decode, closed).await;
                }
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

async fn process_closed<D, F>(
    state: &Arc<TakeState>,
    whole_window: &Mutex<WholeWindowState>,
    guidance: &[String],
    decode: &mut D,
    closed: ClosedRange,
) where
    D: FnMut(DecodeRequest) -> F,
    F: Future<Output = Option<Result<String, TranscribeError>>>,
{
    match closed {
        ClosedRange::Segment {
            range,
            forced,
            leading_silence,
        } => {
            record_leading_silence(state, whole_window, leading_silence);
            let samples = TakeState::lock(&state.buffer)
                .transfer_range(range.clone())
                .unwrap_or_else(|_| panic!("ordered segment range must remain resident"));
            process_samples(
                state,
                whole_window,
                guidance,
                decode,
                samples,
                range,
                forced,
            )
            .await;
        }
        ClosedRange::Skipped {
            range,
            reason,
            leading_silence,
            silent_through,
        } => {
            record_leading_silence(state, whole_window, leading_silence);
            TakeState::lock(&state.buffer)
                .compact_to(range.end)
                .unwrap_or_else(|_| panic!("ordered skipped range must remain resident"));
            record_skipped_segment(
                state,
                whole_window,
                FinalRangeOutcome::skipped(range, reason),
                silent_through,
            );
        }
        ClosedRange::Released {
            range,
            leading_silence,
        } => {
            record_leading_silence(state, whole_window, leading_silence);
            record_outcome(
                state,
                whole_window,
                FinalRangeOutcome::skipped(range, SkipReason::Released),
            );
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

/// Records a click that the segmenter skipped after hearing silence from its
/// end through `silent_through`. Accepted text whose window ran on into that
/// silence holds no word from past the segment, so it counts as ending with
/// the segment and can stand in for it.
fn record_skipped_segment(
    state: &TakeState,
    whole_window: &Mutex<WholeWindowState>,
    outcome: FinalRangeOutcome,
    silent_through: u64,
) {
    let end = outcome.range.end;
    let window = TakeState::lock(whole_window);
    let accepted = window
        .accepted_hypotheses(silent_through.max(end))
        .into_iter()
        .map(|hypothesis| {
            let range = hypothesis.range();
            if range.start < end && range.end > end {
                AcceptedHypothesis::new(range.start..end, hypothesis.text().to_owned())
            } else {
                hypothesis
            }
        })
        .collect::<Vec<_>>();
    let shown = window.shown();
    drop(window);
    state.record_final_outcome(outcome, &accepted, &shown);
}

#[cfg(test)]
mod short_tests;
#[cfg(test)]
mod speech_tests;
#[cfg(test)]
mod tests;
