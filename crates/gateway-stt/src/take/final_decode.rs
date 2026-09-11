use std::future::Future;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use gateway_stt_engine::{DecodeMode, DecodeRequest, EnginePolicy, TranscribeError};
use tokio::sync::oneshot;

use crate::segment::{FORCED_OVERLAP_SAMPLES, ForcedBoundary};

use super::final_outcome::{FinalRangeOutcome, SkipReason};
use super::pcm::RetainedPcm;
use super::state::{TakeFailure, TakeState};
use super::window::WholeWindowState;

pub(super) async fn process_samples<D, F>(
    state: &Arc<TakeState>,
    whole_window: &Mutex<WholeWindowState>,
    guidance: &[String],
    decode: &mut D,
    samples: RetainedPcm,
    range: Range<u64>,
    forced: Option<ForcedBoundary>,
) where
    D: FnMut(DecodeRequest) -> F,
    F: Future<Output = Option<Result<String, TranscribeError>>>,
{
    if state.has_failure() {
        return;
    }
    let skipped = if samples.len() < EnginePolicy::MIN_WINDOW_SAMPLES {
        Some(SkipReason::BelowFinalWindow)
    } else if EnginePolicy::is_silence(samples.samples()) {
        Some(SkipReason::Silence)
    } else {
        None
    };
    if let Some(reason) = skipped {
        if forced.is_some() {
            state.record_failure(TakeFailure::ForcedWindowNotDecodable);
        } else {
            super::finalization::record_outcome(
                state,
                whole_window,
                FinalRangeOutcome::skipped(range, reason),
            );
        }
        return;
    }

    match forced {
        Some(boundary) => {
            process_forced(state, whole_window, guidance, decode, samples, boundary).await;
        }
        None => {
            process_natural(state, whole_window, guidance, decode, samples, range).await;
        }
    }
}

async fn process_natural<D, F>(
    state: &TakeState,
    whole_window: &Mutex<WholeWindowState>,
    guidance: &[String],
    decode: &mut D,
    samples: RetainedPcm,
    range: Range<u64>,
) where
    D: FnMut(DecodeRequest) -> F,
    F: Future<Output = Option<Result<String, TranscribeError>>>,
{
    let finalized = state.finalized();
    let (samples, owner) = samples.into_decode();
    let request = DecodeRequest::new(DecodeMode::Final, samples, guidance.to_vec(), finalized)
        .with_lifetime_guard(owner);
    record_decode(state, whole_window, decode(request).await, |text| {
        FinalRangeOutcome::decoded(range, text)
    });
}

async fn process_forced<D, F>(
    state: &Arc<TakeState>,
    whole_window: &Mutex<WholeWindowState>,
    guidance: &[String],
    decode: &mut D,
    samples: RetainedPcm,
    boundary: ForcedBoundary,
) where
    D: FnMut(DecodeRequest) -> F,
    F: Future<Output = Option<Result<String, TranscribeError>>>,
{
    let finalized = state.finalized();
    let (samples, owner) = samples.into_decode();
    let (returned, receiver) = oneshot::channel();
    let retirement_state = Arc::clone(state);
    let retirement_boundary = boundary.clone();
    let request = DecodeRequest::new(DecodeMode::Final, samples, guidance.to_vec(), finalized)
        .with_sample_retirement(move |samples| {
            let restored = if retirement_boundary.retains_overlap() {
                restore_overlap(&retirement_state, &retirement_boundary, samples, owner)
            } else {
                drop(samples);
                drop(owner);
                true
            };
            let _ = returned.send(restored);
        });
    let outcome = decode(request).await;
    let Ok(restored) = receiver.await else {
        state.record_failure(TakeFailure::RetirementFailed);
        return;
    };
    if !restored {
        state.record_failure(TakeFailure::OwnershipInconsistent);
        return;
    }
    record_decode(state, whole_window, outcome, |text| {
        FinalRangeOutcome::forced(boundary, text)
    });
}

fn restore_overlap(
    state: &TakeState,
    boundary: &ForcedBoundary,
    samples: Vec<f32>,
    owner: super::pcm::RetainedPcmOwner,
) -> bool {
    let new_audio = boundary.new_audio();
    let Some(start) = new_audio.end.checked_sub(FORCED_OVERLAP_SAMPLES as u64) else {
        return false;
    };
    let range = start..new_audio.end;
    let mut buffer = TakeState::lock(&state.buffer);
    if buffer.origin() > range.end {
        return true;
    }
    let overlap = RetainedPcm::retain_tail(samples, owner, FORCED_OVERLAP_SAMPLES);
    buffer.origin() == range.end && buffer.restore_prefix(range, overlap).is_ok()
}

fn record_decode(
    state: &TakeState,
    whole_window: &Mutex<WholeWindowState>,
    outcome: Option<Result<String, TranscribeError>>,
    completed: impl FnOnce(String) -> FinalRangeOutcome,
) {
    match outcome {
        Some(Ok(text)) => {
            super::finalization::record_outcome(state, whole_window, completed(text));
        }
        Some(Err(error)) => state.record_failure(TakeFailure::Transcribe(error)),
        None => state.record_failure(TakeFailure::WorkerUnavailable),
    }
}
