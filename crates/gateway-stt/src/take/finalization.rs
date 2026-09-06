use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gateway_stt_engine::{DecodeMode, DecodeRequest, TranscribeError};
use tokio::sync::{mpsc, oneshot};

use crate::generation::GenerationLease;

use super::state::TakeState;

pub(super) type TakeFinalization = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;
pub(super) const FINAL_SEGMENT_CAPACITY: usize = 4;

#[derive(Debug)]
pub(super) enum FinalCommand {
    Segment {
        samples: Vec<f32>,
        end: usize,
    },
    Complete {
        tail: Vec<f32>,
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
            let segment = {
                let buffer = TakeState::lock(&state.buffer);
                TakeState::lock(&state.segmenter)
                    .poll(&buffer)
                    .map(|range| (buffer[range.clone()].to_vec(), range.end))
            };
            let Some((samples, end)) = segment else {
                break;
            };
            if !reserve_segment(&self.pending_segments) {
                state.record_failure("final segment capacity is reached".to_owned());
                break;
            }
            match self
                .commands
                .try_send(FinalCommand::Segment { samples, end })
            {
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

    pub(super) fn finalization(&self, tail: Vec<f32>) -> TakeFinalization {
        let commands = self.commands.clone();
        Box::pin(async move {
            let (reply, reply_rx) = oneshot::channel();
            if commands
                .send(FinalCommand::Complete { tail, reply })
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
        let (samples, finalized_samples, completion) = match command {
            FinalCommand::Segment { samples, end } => (samples, Some(end), None),
            FinalCommand::Complete { tail, reply } => (tail, None, Some(reply)),
        };
        if !state.has_failure() {
            let finalized = state.finalized();
            match decode(samples, guidance.to_vec(), finalized).await {
                Some(result) => state.record_finalized(result, finalized_samples),
                None => {
                    state.record_failure("final transcription worker is unavailable".to_owned());
                }
            }
        }
        if finalized_samples.is_some() {
            pending_segments.fetch_sub(1, Ordering::AcqRel);
        }
        if let Some(reply) = completion {
            drop(reply.send(state.completion()));
            break;
        }
    }
}
