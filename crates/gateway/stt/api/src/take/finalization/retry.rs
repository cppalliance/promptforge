//! Closed ranges held while the per-take final queue is full.
//!
//! A held range keeps its PCM resident and queues in audio order once the
//! queue has room. When new audio would exceed the retained PCM cap, the
//! oldest held range is released if nothing before it is still resident: its
//! PCM returns to the budget and its accepted interim text becomes final. A
//! forced successor of a released stride loses the overlap it shared, so it
//! decodes as a first window, or as a natural segment when silence closed it.

use std::collections::VecDeque;
use std::ops::Range;

use tokio::sync::mpsc::error::TrySendError;

use super::{FinalCommand, FinalPipeline, FinalSegmentOwner};
use crate::audio::AudioError;
use crate::segment::{ForcedBoundary, SegmentOutcome};
use crate::take::final_outcome::SkipReason;
use crate::take::pcm::RollingPcm;
use crate::take::state::{TakeFailure, TakeState};

/// A closed range on its way to the final pass.
#[derive(Debug)]
pub(in crate::take) enum ClosedRange {
    Segment {
        range: Range<u64>,
        forced: Option<ForcedBoundary>,
        leading_silence: Option<Range<u64>>,
        /// End of the silence the segmenter heard after `range` before it
        /// closed the segment, or the end of `range` when a stride cut it.
        silent_through: u64,
    },
    Skipped {
        range: Range<u64>,
        reason: SkipReason,
        leading_silence: Option<Range<u64>>,
        /// End of the silence the segmenter heard after `range` before it
        /// closed the segment.
        silent_through: u64,
    },
    /// Released under the PCM cap before a queue slot opened; its PCM is
    /// gone and its accepted interim text is final.
    Released {
        range: Range<u64>,
        leading_silence: Option<Range<u64>>,
        /// End of the accepted windows that may stand in for `range`.
        text_through: u64,
    },
}

impl ClosedRange {
    fn closed(outcome: SegmentOutcome, previous_consumed: u64, scanned: u64) -> Self {
        let range = match &outcome {
            SegmentOutcome::Decode(range) | SegmentOutcome::Skipped(range) => range.clone(),
            SegmentOutcome::Forced(boundary) => boundary.decode_range(),
        };
        let leading_silence =
            (previous_consumed < range.start).then_some(previous_consumed..range.start);
        match outcome {
            SegmentOutcome::Decode(_) => Self::Segment {
                range,
                forced: None,
                leading_silence,
                silent_through: scanned,
            },
            SegmentOutcome::Forced(boundary) => Self::Segment {
                range,
                forced: Some(boundary),
                leading_silence,
                silent_through: scanned,
            },
            SegmentOutcome::Skipped(_) => Self::Skipped {
                range,
                reason: SkipReason::BelowSpeechThreshold,
                leading_silence,
                silent_through: scanned,
            },
        }
    }

    /// Whether releasing frees this range's PCM now: every earlier range must
    /// have left the buffer.
    fn releasable(&self, origin: u64) -> bool {
        match self {
            Self::Segment {
                range,
                leading_silence,
                ..
            }
            | Self::Skipped {
                range,
                leading_silence,
                ..
            } => {
                origin
                    >= leading_silence
                        .as_ref()
                        .map_or(range.start, |silence| silence.start)
            }
            Self::Released { .. } => false,
        }
    }

    fn range(&self) -> Range<u64> {
        let (Self::Segment { range, .. }
        | Self::Skipped { range, .. }
        | Self::Released { range, .. }) = self;
        range.clone()
    }

    /// A released forced window keeps only its new audio, because its
    /// predecessor's final text covers the overlap. Accepted text running
    /// past a silence close ran on only into that silence, and past a stride
    /// cut into audio the successor decodes, so either stands in for the
    /// released range.
    fn release(&mut self) {
        let (range, text_through) = match self {
            Self::Segment {
                forced: Some(boundary),
                silent_through,
                ..
            } => (
                boundary.new_audio(),
                if boundary.retains_overlap() {
                    u64::MAX
                } else {
                    *silent_through
                },
            ),
            Self::Segment {
                range,
                silent_through,
                ..
            }
            | Self::Skipped {
                range,
                silent_through,
                ..
            } => (range.clone(), *silent_through),
            Self::Released {
                range,
                text_through,
                ..
            } => (range.clone(), *text_through),
        };
        let (Self::Segment {
            leading_silence, ..
        }
        | Self::Skipped {
            leading_silence, ..
        }
        | Self::Released {
            leading_silence, ..
        }) = self;
        let released = Self::Released {
            range,
            leading_silence: leading_silence.take(),
            text_through,
        };
        *self = released;
    }

    /// Detaches a forced successor of the released stride that ended at `end`.
    fn detach_from(&mut self, end: u64) {
        let Self::Segment { range, forced, .. } = self else {
            return;
        };
        let Some(boundary) = forced
            .as_ref()
            .filter(|boundary| boundary.overlap().is_some_and(|overlap| overlap.end == end))
        else {
            return;
        };
        let new_audio = boundary.new_audio();
        let stride = boundary.retains_overlap();
        *forced = stride.then(|| ForcedBoundary::first(new_audio.clone()));
        *range = new_audio;
    }
}

enum Submission {
    Queued,
    Full(ClosedRange),
    Exited,
}

impl FinalPipeline {
    /// Queues held ranges in audio order while the queue has room, then each
    /// newly closed range, holding it behind any range still waiting.
    pub(in crate::take) fn submit_closed_segments(&self, state: &TakeState) {
        let mut held = TakeState::lock(&state.held);
        while let Some(closed) = held.pop_front() {
            match self.try_submit(closed) {
                Submission::Queued => {}
                Submission::Full(closed) => {
                    held.push_front(closed);
                    break;
                }
                Submission::Exited => {
                    state.record_failure(TakeFailure::PipelineExited);
                    return;
                }
            }
        }
        while let Some(closed) = poll_closed(state) {
            if !held.is_empty() {
                held.push_back(closed);
                continue;
            }
            match self.try_submit(closed) {
                Submission::Queued => {}
                Submission::Full(closed) => held.push_back(closed),
                Submission::Exited => {
                    state.record_failure(TakeFailure::PipelineExited);
                    return;
                }
            }
        }
    }

    fn try_submit(&self, closed: ClosedRange) -> Submission {
        let Some(owner) = FinalSegmentOwner::reserve(&self.pending_segments) else {
            return Submission::Full(closed);
        };
        match self.commands.try_reserve() {
            Ok(permit) => {
                permit.send(FinalCommand::Closed { closed, owner });
                Submission::Queued
            }
            Err(TrySendError::Full(())) => Submission::Full(closed),
            Err(TrySendError::Closed(())) => Submission::Exited,
        }
    }
}

fn poll_closed(state: &TakeState) -> Option<ClosedRange> {
    let mut segmenter = TakeState::lock(&state.segmenter);
    let previous_consumed = segmenter.consumed();
    let outcome = segmenter.poll()?;
    Some(ClosedRange::closed(
        outcome,
        previous_consumed,
        segmenter.scanned(),
    ))
}

/// Appends `samples`, releasing the oldest held range each time the retained
/// PCM cap would otherwise reject them.
pub(in crate::take) fn append_releasing(
    state: &TakeState,
    mut samples: Vec<f32>,
) -> Result<(), AudioError> {
    let mut held = TakeState::lock(&state.held);
    let mut buffer = TakeState::lock(&state.buffer);
    loop {
        match buffer.try_append(samples) {
            Ok(()) => return Ok(()),
            Err((_, rejected)) if release_oldest(state, &mut held, &mut buffer) => {
                samples = rejected;
            }
            Err((error, _)) => return Err(error),
        }
    }
}

fn releasable_index(held: &VecDeque<ClosedRange>, origin: u64) -> Option<usize> {
    held.iter()
        .position(|closed| !matches!(closed, ClosedRange::Released { .. }))
        .filter(|index| held[*index].releasable(origin))
}

fn release_oldest(
    state: &TakeState,
    held: &mut VecDeque<ClosedRange>,
    buffer: &mut RollingPcm,
) -> bool {
    let Some(index) = releasable_index(held, buffer.origin()) else {
        return false;
    };
    let end = held[index].range().end;
    if buffer.release_prefix(end).is_err() {
        return false;
    }
    held[index].release();
    match held.get_mut(index + 1) {
        Some(next) => next.detach_from(end),
        None => TakeState::lock(&state.segmenter).forget_forced_predecessor(end),
    }
    true
}
