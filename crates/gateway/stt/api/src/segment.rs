//! Energy-based voice activity segmentation for the pipelined final pass.
//!
//! [`Segmenter`] scans a growing take buffer in fixed frames and reports a
//! completed speech segment each time a run of silence long enough to be a
//! segment boundary follows speech. The session hands each reported range to
//! the final-pass worker while the take is still recording, so on `stop`
//! only the unclosed tail remains to transcribe. The detector is a plain
//! RMS-over-window gate (the same threshold as the interim silence gate);
//! whisper.cpp's own `vad.cpp` Silero integration was considered and
//! rejected as too heavy for this pipeline (see the design log).

use std::ops::Range;

use gateway_stt_engine::EnginePolicy;

mod boundary;
mod endpoint;

pub(crate) use boundary::{ForcedBoundary, SegmentOutcome};
use endpoint::{Closed, EndpointState, Rule, Scan};

/// Analysis frame length: 30 ms at 16 kHz, whisper.cpp's own VAD frame.
pub(crate) const FRAME_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * 30 / 1000;
pub(crate) const FORCED_OVERLAP_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * 8;

/// Speech shorter than 250 ms is discarded as a click or cough rather than
/// transcribed, where whisper would hallucinate a word for it.
const MIN_SPEECH_SAMPLES: usize = EnginePolicy::SAMPLE_RATE / 4;

/// Incremental speech segmenter over one take's PCM buffer.
///
/// The buffer is append-only for the life of a take, so the segmenter keeps
/// a cursor into it and each [`poll`](Segmenter::poll) scans only frames
/// completed since the last call. Ranges are indices into that buffer.
#[derive(Debug, Default)]
pub(crate) struct Segmenter {
    /// Next unscanned sample index.
    cursor: u64,
    /// The tracked speech run and the end of the last completed segment.
    endpoint: EndpointState,
    /// End of the preceding forced stride when uninterrupted speech may
    /// overlap it.
    forced_predecessor: Option<u64>,
    /// Start of the segment whose latest accepted interim text ends a
    /// sentence, if one does.
    sentence_end: Option<u64>,
}

impl Segmenter {
    /// A fresh segmenter positioned at the start of a take buffer.
    #[must_use]
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn new() -> Self {
        Self::default()
    }
    /// Rewinds the segmenter for a new take; the caller clears the buffer at
    /// the same time, so indices stay aligned.
    #[cfg(test)]
    fn reset(&mut self) {
        *self = Self::new();
    }

    #[cfg(test)]
    pub(crate) fn set_consumed_for_test(&mut self, consumed: u64) {
        self.endpoint.consumed = consumed;
    }

    /// Index past which all audio has been segmented; the unprocessed tail
    /// of the take is `buffer[self.consumed()..]`.
    #[must_use]
    pub(crate) fn consumed(&self) -> u64 {
        self.endpoint.consumed
    }

    /// Records whether the latest accepted interim text, decoded from the
    /// segment that starts at `segment_start`, ends a sentence. The hint
    /// shortens the closing silence only while that segment is still open.
    pub(crate) fn set_sentence_end(&mut self, segment_start: u64, ends_sentence: bool) {
        self.sentence_end = ends_sentence.then_some(segment_start);
    }

    /// Whether the open segment's latest accepted interim text ends a sentence.
    pub(crate) fn ends_sentence(&self) -> bool {
        self.sentence_end == Some(self.endpoint.consumed)
    }

    /// Stops the next segment from overlapping the forced stride that ended
    /// at `end`, whose audio was released without a final decode.
    pub(crate) fn forget_forced_predecessor(&mut self, end: u64) {
        if self.forced_predecessor == Some(end) {
            self.forced_predecessor = None;
        }
    }

    pub(crate) fn terminal_boundary(&self, end: u64) -> Option<ForcedBoundary> {
        let consumed = self.endpoint.consumed;
        (consumed < end && self.forced_predecessor == Some(consumed))
            .then(|| Self::successor(consumed..end, false))
    }

    /// Scans newly arrived frames and returns the range of the next
    /// completed speech segment, if one closed. Call in a loop: a large
    /// arrival can complete more than one segment.
    pub(crate) fn poll(&mut self, buffer: &[f32], buffer_origin: u64) -> Option<SegmentOutcome> {
        debug_assert!(self.cursor >= buffer_origin);
        let received =
            buffer_origin.checked_add(u64::try_from(buffer.len()).unwrap_or(u64::MAX))?;
        loop {
            let frame = usize::try_from(self.cursor - buffer_origin)
                .ok()
                .and_then(|start| buffer.get(start..start.checked_add(FRAME_SAMPLES)?));
            let scan = Scan {
                cursor: self.cursor,
                received,
                silent: frame.map(EnginePolicy::is_silence),
                sentence_end: self.ends_sentence(),
            };
            let advance = endpoint::endpoint(self.endpoint, scan)?;
            self.cursor = advance.cursor;
            self.endpoint = advance.state;
            if let Some(closed) = advance.closed {
                return Some(self.outcome(closed));
            }
        }
    }

    /// Classifies a closed segment. Overlap ownership, the click rule, and the
    /// final-window floor read the speech run, so padding never turns a click
    /// or a burst shorter than the final window into a decode, or a resumed
    /// run into a successor of the stride before its pause.
    fn outcome(&mut self, closed: Closed) -> SegmentOutcome {
        let Closed {
            rule,
            speech,
            segment,
        } = closed;
        let continues = self.forced_predecessor == Some(speech.start);
        debug_assert!(!continues || segment.start == speech.start);
        self.forced_predecessor = (rule == Rule::Stride).then_some(segment.end);
        let speech_samples = speech.end - speech.start;
        match rule {
            Rule::Stride if continues => SegmentOutcome::Forced(Self::successor(segment, true)),
            Rule::Stride => SegmentOutcome::Forced(ForcedBoundary::first(segment)),
            Rule::Silence if speech_samples < MIN_SPEECH_SAMPLES as u64 => {
                SegmentOutcome::Skipped(segment)
            }
            Rule::Silence if continues => SegmentOutcome::Forced(Self::successor(segment, false)),
            Rule::Silence if speech_samples < EnginePolicy::MIN_WINDOW_SAMPLES as u64 => {
                SegmentOutcome::Skipped(segment)
            }
            Rule::Silence => SegmentOutcome::Decode(segment),
        }
    }

    fn successor(new_audio: Range<u64>, retain_overlap: bool) -> ForcedBoundary {
        let overlap_start = new_audio
            .start
            .checked_sub(FORCED_OVERLAP_SAMPLES as u64)
            .unwrap_or_else(|| unreachable!("a forced stride is longer than its overlap"));
        if retain_overlap {
            ForcedBoundary::overlapping(overlap_start..new_audio.start, new_audio)
        } else {
            ForcedBoundary::overlapping_final(overlap_start..new_audio.start, new_audio)
        }
    }
}

#[cfg(test)]
mod tests;
