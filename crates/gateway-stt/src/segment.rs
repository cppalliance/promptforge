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

pub(crate) use boundary::{ForcedBoundary, SegmentOutcome};

/// Analysis frame length: 30 ms at 16 kHz, whisper.cpp's own VAD frame.
const FRAME_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * 30 / 1000;
const FORCED_STRIDE_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE * 10) as u64;
pub(crate) const FORCED_OVERLAP_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * 8;

/// Silence must persist this long after speech to close a segment: 700 ms,
/// long enough to survive sentence-internal pauses and natural breathing
/// gaps (~2 s), short enough that the final pass starts well before the
/// user stops talking.
const MIN_SILENCE_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * 2;

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
    /// Start of the speech run currently being tracked, if any.
    speech_start: Option<u64>,
    /// Start of the silent run following the tracked speech, if one began.
    silence_begin: Option<u64>,
    /// End of the last completed segment: everything before this index has
    /// been handed to the final pass (or discarded as a click).
    consumed: u64,
    /// End of the preceding forced stride when uninterrupted speech may
    /// overlap it.
    forced_predecessor: Option<u64>,
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
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    #[cfg(test)]
    pub(crate) fn set_consumed_for_test(&mut self, consumed: u64) {
        self.consumed = consumed;
    }

    /// Index past which all audio has been segmented; the unprocessed tail
    /// of the take is `buffer[self.consumed()..]`.
    #[must_use]
    pub(crate) fn consumed(&self) -> u64 {
        self.consumed
    }

    pub(crate) fn terminal_boundary(&self, end: u64) -> Option<ForcedBoundary> {
        (self.consumed < end)
            .then(|| self.overlapping_successor(self.consumed..end, false))
            .flatten()
    }

    /// Scans newly arrived frames and returns the range of the next
    /// completed speech segment, if one closed. Call in a loop: a large
    /// arrival can complete more than one segment.
    pub(crate) fn poll(&mut self, buffer: &[f32], buffer_origin: u64) -> Option<SegmentOutcome> {
        debug_assert!(self.cursor >= buffer_origin);
        let frame_samples = FRAME_SAMPLES as u64;
        let buffer_end =
            buffer_origin.checked_add(u64::try_from(buffer.len()).unwrap_or(u64::MAX))?;
        loop {
            if let Some(start) = self.speech_start {
                let forced_end = start.checked_add(FORCED_STRIDE_SAMPLES)?;
                if forced_end <= buffer_end
                    && self.cursor.saturating_add(frame_samples) > forced_end
                {
                    return Some(self.force_boundary(start, forced_end));
                }
            }
            if self.cursor.saturating_add(frame_samples) > buffer_end {
                return None;
            }
            let start = usize::try_from(self.cursor - buffer_origin).ok()?;
            let frame = &buffer[start..start + FRAME_SAMPLES];
            let silent = EnginePolicy::is_silence(frame);
            match (self.speech_start, silent) {
                (Some(start), true) => {
                    let begin = self.silence_begin.get_or_insert(self.cursor);
                    if self.cursor + frame_samples - *begin >= MIN_SILENCE_SAMPLES as u64 {
                        let end = *begin;
                        self.speech_start = None;
                        self.silence_begin = None;
                        self.cursor += frame_samples;
                        self.consumed = end;
                        if end - start >= MIN_SPEECH_SAMPLES as u64 {
                            let range = start..end;
                            let outcome = self
                                .overlapping_successor(range.clone(), false)
                                .map_or(SegmentOutcome::Decode(range), SegmentOutcome::Forced);
                            self.forced_predecessor = None;
                            return Some(outcome);
                        }
                        self.forced_predecessor = None;
                        return Some(SegmentOutcome::Skipped(start..end));
                    }
                }
                (None, false) => {
                    self.speech_start = Some(self.cursor);
                }
                (Some(_), false) => {
                    self.silence_begin = None;
                }
                (None, true) => {}
            }
            self.cursor += frame_samples;
        }
    }

    fn force_boundary(&mut self, start: u64, end: u64) -> SegmentOutcome {
        let boundary = if self.forced_predecessor == Some(start) {
            ForcedBoundary::overlapping(start - FORCED_OVERLAP_SAMPLES as u64..start, start..end)
        } else {
            ForcedBoundary::first(start..end)
        };
        self.speech_start = self.silence_begin.is_none().then_some(end);
        self.silence_begin = None;
        self.cursor = end;
        self.consumed = end;
        self.forced_predecessor = Some(end);
        SegmentOutcome::Forced(boundary)
    }

    fn overlapping_successor(
        &self,
        new_audio: Range<u64>,
        retain_overlap: bool,
    ) -> Option<ForcedBoundary> {
        (self.forced_predecessor == Some(new_audio.start)).then(|| {
            let overlap_start = new_audio
                .start
                .checked_sub(FORCED_OVERLAP_SAMPLES as u64)
                .unwrap_or_else(|| unreachable!("a forced stride is longer than its overlap"));
            if retain_overlap {
                ForcedBoundary::overlapping(overlap_start..new_audio.start, new_audio)
            } else {
                ForcedBoundary::overlapping_final(overlap_start..new_audio.start, new_audio)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of loud synthetic speech (a constant 0.5 tone).
    fn speech(seconds: usize) -> Vec<f32> {
        vec![0.5; seconds * EnginePolicy::SAMPLE_RATE]
    }

    /// One second of digital silence.
    fn silence(seconds: usize) -> Vec<f32> {
        vec![0.0; seconds * EnginePolicy::SAMPLE_RATE]
    }

    /// Concatenates blocks of speech and silence into one buffer.
    fn take(blocks: &[Vec<f32>]) -> Vec<f32> {
        blocks.concat()
    }

    /// Drains every segment the segmenter can close over `buffer`.
    fn close_all(segmenter: &mut Segmenter, buffer: &[f32]) -> Vec<Range<u64>> {
        let mut ranges = Vec::new();
        while let Some(outcome) = segmenter.poll(buffer, 0) {
            match outcome {
                SegmentOutcome::Decode(range) => ranges.push(range),
                SegmentOutcome::Forced(boundary) => ranges.push(boundary.decode_range()),
                SegmentOutcome::Skipped(_) => {}
            }
        }
        ranges
    }

    #[test]
    fn silence_only_yields_no_segment() {
        let buffer = silence(5);
        let mut segmenter = Segmenter::new();
        assert!(close_all(&mut segmenter, &buffer).is_empty());
        assert_eq!(segmenter.consumed(), 0);
    }

    #[test]
    fn ongoing_speech_does_not_close() {
        let buffer = speech(5);
        let mut segmenter = Segmenter::new();
        assert!(
            close_all(&mut segmenter, &buffer).is_empty(),
            "a segment closes only on trailing silence"
        );
        assert_eq!(segmenter.consumed(), 0);
    }

    #[test]
    fn speech_closes_after_enough_silence() {
        let buffer = take(&[speech(2), silence(3)]);
        let mut segmenter = Segmenter::new();
        let ranges = close_all(&mut segmenter, &buffer);
        assert_eq!(ranges.len(), 1, "one speech run closes one segment");
        let range = &ranges[0];
        assert_eq!(range.start, 0);
        assert!(
            range.end <= (2 * EnginePolicy::SAMPLE_RATE + FRAME_SAMPLES) as u64,
            "the segment ends where the silence began: {range:?}"
        );
        assert!(
            range.end - range.start >= (2 * EnginePolicy::SAMPLE_RATE - FRAME_SAMPLES) as u64,
            "the segment holds the whole speech run: {range:?}"
        );
        assert_eq!(segmenter.consumed(), range.end);
    }

    #[test]
    fn a_short_pause_does_not_close_the_segment() {
        // One second of silence is inside the 2 s closing threshold.
        let buffer = take(&[speech(1), silence(1), speech(1)]);
        let mut segmenter = Segmenter::new();
        assert!(
            close_all(&mut segmenter, &buffer).is_empty(),
            "a sentence-internal pause must not split the segment"
        );
    }

    #[test]
    fn clicks_shorter_than_min_speech_are_discarded() {
        // 100 ms of tone followed by a full closing silence.
        let buffer = take(&[
            speech(1)
                .split_at(EnginePolicy::SAMPLE_RATE / 10)
                .0
                .to_vec(),
            silence(3),
        ]);
        let mut segmenter = Segmenter::new();
        let outcome = segmenter
            .poll(&buffer, 0)
            .expect("the discarded click is an explicit outcome");
        assert_eq!(
            outcome,
            SegmentOutcome::Skipped(0..(EnginePolicy::SAMPLE_RATE * 3 / 25) as u64),
            "the frame-aligned click coverage is retained for reconciliation"
        );
        assert!(
            segmenter.consumed() > 0,
            "the click is still consumed so the tail excludes it"
        );
    }

    #[test]
    fn two_speech_runs_close_as_two_segments() {
        let buffer = take(&[speech(1), silence(3), speech(1), silence(3)]);
        let mut segmenter = Segmenter::new();
        let ranges = close_all(&mut segmenter, &buffer);
        assert_eq!(ranges.len(), 2, "each speech run closes its own segment");
        assert!(
            ranges[0].end <= ranges[1].start,
            "segments are ordered and disjoint: {ranges:?}"
        );
        assert_eq!(segmenter.consumed(), ranges[1].end);
    }

    #[test]
    fn poll_is_incremental_over_a_growing_buffer() {
        let mut buffer = speech(1);
        let mut segmenter = Segmenter::new();
        assert!(segmenter.poll(&buffer, 0).is_none());
        buffer.extend_from_slice(&silence(3));
        let SegmentOutcome::Decode(first) = segmenter.poll(&buffer, 0).expect("the segment closes")
        else {
            panic!("ordinary speech is decoded");
        };
        assert_eq!(first.start, 0);
        // Polling again without new audio returns nothing.
        assert!(segmenter.poll(&buffer, 0).is_none());
    }

    #[test]
    fn reset_rewinds_for_a_new_take() {
        let buffer = take(&[speech(1), silence(3)]);
        let mut segmenter = Segmenter::new();
        assert!(segmenter.poll(&buffer, 0).is_some());
        segmenter.reset();
        assert_eq!(segmenter.consumed(), 0);
        assert!(
            segmenter.poll(&buffer, 0).is_some(),
            "after reset the same buffer segments again"
        );
    }

    #[test]
    fn compacted_buffers_keep_absolute_segment_ranges() {
        let mut buffer = take(&[speech(1), silence(3)]);
        let mut segmenter = Segmenter::new();
        let SegmentOutcome::Decode(first) = segmenter
            .poll(&buffer, 0)
            .expect("the first absolute segment closes")
        else {
            panic!("ordinary speech decodes");
        };
        let first_end = usize::try_from(first.end).expect("test range fits");
        buffer.drain(..first_end);
        buffer.extend(take(&[speech(1), silence(3)]));

        let SegmentOutcome::Decode(second) = segmenter
            .poll(&buffer, first.end)
            .expect("the compacted segment closes")
        else {
            panic!("ordinary speech decodes");
        };
        let _: Range<u64> = second.clone();
        assert!(second.start >= first.end);
        assert_eq!(segmenter.consumed(), second.end);
    }

    #[test]
    fn continuous_speech_forces_exact_absolute_strides_with_bounded_overlap() {
        let buffer = speech(20);
        let mut segmenter = Segmenter::new();

        let SegmentOutcome::Forced(first) = segmenter
            .poll(&buffer, 0)
            .expect("ten seconds forces the first final window")
        else {
            panic!("continuous speech uses an explicit forced boundary");
        };
        assert_eq!(first.decode_range(), 0..160_000);
        assert_eq!(first.new_audio(), 0..160_000);
        assert_eq!(first.overlap(), None);
        assert_eq!(
            first.decode_range().end - first.decode_range().start,
            160_000
        );

        let SegmentOutcome::Forced(second) = segmenter
            .poll(&buffer, 0)
            .expect("the next ten seconds force another final window")
        else {
            panic!("later continuous speech retains forced metadata");
        };
        assert_eq!(second.decode_range(), 32_000..320_000);
        assert_eq!(second.new_audio(), 160_000..320_000);
        assert_eq!(second.overlap(), Some(32_000..160_000));
        assert_eq!(
            second.decode_range().end - second.decode_range().start,
            288_000
        );
        assert_eq!(segmenter.consumed(), 320_000);
    }

    #[test]
    fn first_natural_boundary_carries_then_resets_forced_overlap_ownership() {
        let buffer = take(&[speech(10), speech(1), silence(3), speech(10)]);
        let mut segmenter = Segmenter::new();
        assert!(matches!(
            segmenter.poll(&buffer, 0),
            Some(SegmentOutcome::Forced(_))
        ));
        let SegmentOutcome::Forced(natural) = segmenter
            .poll(&buffer, 0)
            .expect("the first natural boundary retains forced overlap")
        else {
            panic!("the forced successor carries reconciliation metadata");
        };
        assert_eq!(natural.overlap(), Some(32_000..160_000));
        assert!(natural.new_audio().start == 160_000);
        assert!(natural.new_audio().end < 320_000);

        let SegmentOutcome::Forced(after_silence) = segmenter
            .poll(&buffer, 0)
            .expect("the next continuous run reaches its own forced boundary")
        else {
            panic!("speech after a natural boundary is forced independently");
        };
        assert_eq!(after_silence.overlap(), None);
        assert_eq!(after_silence.decode_range(), after_silence.new_audio());
    }
}
