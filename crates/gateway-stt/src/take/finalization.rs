use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gateway_stt_engine::{DecodeMode, DecodeRequest, EnginePolicy, TranscribeError};
use tokio::sync::{mpsc, oneshot};

use crate::generation::GenerationLease;
use crate::segment::SegmentOutcome;

use super::final_outcome::{FinalRangeOutcome, SkipReason};
use super::state::TakeState;
use super::window::AcceptedHypothesis;

pub(super) type TakeFinalization = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
pub(super) const FINAL_SEGMENT_CAPACITY: usize = 4;

#[derive(Debug)]
pub(super) enum FinalCommand {
    Segment {
        samples: Vec<f32>,
        range: Range<usize>,
        leading_silence: Option<Range<usize>>,
    },
    Skipped {
        range: Range<usize>,
        reason: SkipReason,
        leading_silence: Option<Range<usize>>,
    },
    Complete {
        tail: Vec<f32>,
        start: usize,
        committed_samples: usize,
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
            let outcome = {
                let buffer = TakeState::lock(&state.buffer);
                let mut segmenter = TakeState::lock(&state.segmenter);
                let previous_consumed = segmenter.consumed();
                segmenter.poll(&buffer).map(|outcome| {
                    let range = match &outcome {
                        SegmentOutcome::Decode(range) | SegmentOutcome::Skipped(range) => range,
                    };
                    let leading_silence =
                        (previous_consumed < range.start).then(|| previous_consumed..range.start);
                    match outcome {
                        SegmentOutcome::Decode(range) => FinalCommand::Segment {
                            samples: buffer[range.clone()].to_vec(),
                            range,
                            leading_silence,
                        },
                        SegmentOutcome::Skipped(range) => FinalCommand::Skipped {
                            range,
                            reason: SkipReason::BelowSpeechThreshold,
                            leading_silence,
                        },
                    }
                })
            };
            let Some(command) = outcome else {
                break;
            };
            if !reserve_segment(&self.pending_segments) {
                state.record_failure("final segment capacity is reached".to_owned());
                break;
            }
            match self.commands.try_send(command) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    self.pending_segments.fetch_sub(1, Ordering::AcqRel);
                    state.record_failure("final segment capacity is reached".to_owned());
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.pending_segments.fetch_sub(1, Ordering::AcqRel);
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
        tail: Vec<f32>,
        start: usize,
        committed_samples: usize,
        accepted: Vec<AcceptedHypothesis>,
    ) -> TakeFinalization {
        let commands = self.commands.clone();
        Box::pin(async move {
            let (reply, reply_rx) = oneshot::channel();
            if commands
                .send(FinalCommand::Complete {
                    tail,
                    start,
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

pub(super) fn reserve_segment(pending_segments: &AtomicUsize) -> bool {
    pending_segments
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
            (pending < FINAL_SEGMENT_CAPACITY).then_some(pending + 1)
        })
        .is_ok()
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
        Arc::clone(&pending_segments),
        move |samples, guidance, finalized| {
            let engine = engine.clone();
            async move {
                if !engine.has_final_pass() {
                    return None;
                }
                Some(
                    engine
                        .decode(DecodeRequest::new(
                            DecodeMode::Final,
                            samples,
                            guidance,
                            finalized,
                        ))
                        .await,
                )
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
    pending_segments: Arc<AtomicUsize>,
    mut decode: D,
) where
    D: FnMut(Vec<f32>, Vec<String>, String) -> F,
    F: Future<Output = Option<Result<String, TranscribeError>>>,
{
    while let Some(command) = receiver.recv().await {
        match command {
            FinalCommand::Segment {
                samples,
                range,
                leading_silence,
            } => {
                record_leading_silence(&state, leading_silence);
                process_samples(&state, &guidance, &mut decode, samples, range).await;
                pending_segments.fetch_sub(1, Ordering::AcqRel);
            }
            FinalCommand::Skipped {
                range,
                reason,
                leading_silence,
            } => {
                record_leading_silence(&state, leading_silence);
                state.record_final_outcome(FinalRangeOutcome::skipped(range, reason));
                pending_segments.fetch_sub(1, Ordering::AcqRel);
            }
            FinalCommand::Complete {
                tail,
                start,
                committed_samples,
                accepted,
                reply,
            } => {
                process_samples(
                    &state,
                    &guidance,
                    &mut decode,
                    tail,
                    start..committed_samples,
                )
                .await;
                drop(reply.send(state.completion(&accepted, committed_samples)));
                break;
            }
        }
    }
}

fn record_leading_silence(state: &TakeState, range: Option<Range<usize>>) {
    if let Some(range) = range {
        state.record_final_outcome(FinalRangeOutcome::skipped(range, SkipReason::Silence));
    }
}

async fn process_samples<D, F>(
    state: &TakeState,
    guidance: &[String],
    decode: &mut D,
    samples: Vec<f32>,
    range: Range<usize>,
) where
    D: FnMut(Vec<f32>, Vec<String>, String) -> F,
    F: Future<Output = Option<Result<String, TranscribeError>>>,
{
    if state.has_failure() {
        return;
    }
    let skipped = if samples.len() < EnginePolicy::MIN_WINDOW_SAMPLES {
        Some(SkipReason::BelowFinalWindow)
    } else if EnginePolicy::is_silence(&samples) {
        Some(SkipReason::Silence)
    } else {
        None
    };
    if let Some(reason) = skipped {
        state.record_final_outcome(FinalRangeOutcome::skipped(range, reason));
        return;
    }
    let finalized = state.finalized();
    match decode(samples, guidance.to_vec(), finalized).await {
        Some(Ok(text)) => {
            state.record_final_outcome(FinalRangeOutcome::decoded(range, text));
        }
        Some(Err(error)) => state.record_failure(error.to_string()),
        None => {
            state.record_failure("final transcription worker is unavailable".to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::{mpsc, oneshot};

    use super::{FinalCommand, run_final_pipeline};
    use crate::take::Take;
    use crate::take::state::TakeState;
    use crate::take::window::AcceptedHypothesis;

    fn accepted_from_snapshot(
        take: &Take,
        range: std::ops::Range<usize>,
        text: &str,
        committed_samples: usize,
    ) -> Vec<AcceptedHypothesis> {
        take.next_window_snapshot(text, range.start, range.start, range.end)
            .expect("the production window snapshot is accepted");
        TakeState::lock(&take.whole_window).accepted_hypotheses(committed_samples)
    }

    #[tokio::test]
    async fn pipeline_reconciles_short_tail_without_decoding_it() {
        let (commands, receiver) = mpsc::channel(1);
        let take = Take::without_final(Vec::new());
        take.append(&vec![0.5; 4_800]);
        let accepted = accepted_from_snapshot(&take, 0..4_800, "last word", 4_800);
        let state = Arc::clone(&take.state);
        let calls = Arc::new(AtomicUsize::new(0));
        let decode_calls = Arc::clone(&calls);
        let task = tokio::spawn(run_final_pipeline(
            receiver,
            Arc::from([]),
            state,
            Arc::new(AtomicUsize::new(0)),
            move |_, _, _| {
                decode_calls.fetch_add(1, Ordering::SeqCst);
                async { Some(Ok("must not decode".to_owned())) }
            },
        ));
        let (reply, completion) = oneshot::channel();
        commands
            .send(FinalCommand::Complete {
                tail: vec![0.5; 4_800],
                start: 0,
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
        take.append(&vec![0.5; 8_000]);
        let accepted = accepted_from_snapshot(&take, 0..8_000, "must not inherit", 8_000);
        let state = Arc::clone(&take.state);
        let calls = Arc::new(AtomicUsize::new(0));
        let decode_calls = Arc::clone(&calls);
        let task = tokio::spawn(run_final_pipeline(
            receiver,
            Arc::from([]),
            state,
            Arc::new(AtomicUsize::new(0)),
            move |_, _, _| {
                decode_calls.fetch_add(1, Ordering::SeqCst);
                async { Some(Ok("must not decode".to_owned())) }
            },
        ));
        let (reply, completion) = oneshot::channel();
        commands
            .send(FinalCommand::Complete {
                tail: vec![0.5; 4_000],
                start: 4_000,
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
