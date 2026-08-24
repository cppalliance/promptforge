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

use crate::transcribe::{self, SAMPLE_RATE};

/// Analysis frame length: 30 ms at 16 kHz, whisper.cpp's own VAD frame.
const FRAME_SAMPLES: usize = SAMPLE_RATE * 30 / 1000;

/// Silence must persist this long after speech to close a segment: 700 ms,
/// long enough to survive sentence-internal pauses, short enough that the
/// final pass starts well before the user stops talking.
const MIN_SILENCE_SAMPLES: usize = SAMPLE_RATE * 700 / 1000;

/// Speech shorter than 250 ms is discarded as a click or cough rather than
/// transcribed, where whisper would hallucinate a word for it.
const MIN_SPEECH_SAMPLES: usize = SAMPLE_RATE / 4;

/// Incremental speech segmenter over one take's PCM buffer.
///
/// The buffer is append-only for the life of a take, so the segmenter keeps
/// a cursor into it and each [`poll`](Segmenter::poll) scans only frames
/// completed since the last call. Ranges are indices into that buffer.
#[derive(Debug, Default)]
pub(crate) struct Segmenter {
    /// Next unscanned sample index.
    cursor: usize,
    /// Start of the speech run currently being tracked, if any.
    speech_start: Option<usize>,
    /// Start of the silent run following the tracked speech, if one began.
    silence_begin: Option<usize>,
    /// End of the last completed segment: everything before this index has
    /// been handed to the final pass (or discarded as a click).
    consumed: usize,
}

impl Segmenter {
    /// A fresh segmenter positioned at the start of a take buffer.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Rewinds the segmenter for a new take; the caller clears the buffer at
    /// the same time, so indices stay aligned.
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    /// Index past which all audio has been segmented; the unprocessed tail
    /// of the take is `buffer[self.consumed()..]`.
    pub(crate) fn consumed(&self) -> usize {
        self.consumed
    }

    /// Scans newly arrived frames and returns the range of the next
    /// completed speech segment, if one closed. Call in a loop: a large
    /// arrival can complete more than one segment.
    pub(crate) fn poll(&mut self, buffer: &[f32]) -> Option<Range<usize>> {
        while self.cursor + FRAME_SAMPLES <= buffer.len() {
            let frame = &buffer[self.cursor..self.cursor + FRAME_SAMPLES];
            let silent = transcribe::is_silence(frame);
            match (self.speech_start, silent) {
                (Some(start), true) => {
                    let begin = self.silence_begin.get_or_insert(self.cursor);
                    if self.cursor + FRAME_SAMPLES - *begin >= MIN_SILENCE_SAMPLES {
                        let end = *begin;
                        self.speech_start = None;
                        self.silence_begin = None;
                        self.cursor += FRAME_SAMPLES;
                        self.consumed = end;
                        if end - start >= MIN_SPEECH_SAMPLES {
                            return Some(start..end);
                        }
                        // A click: consumed past it, nothing to transcribe.
                        continue;
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
            self.cursor += FRAME_SAMPLES;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of loud synthetic speech (a constant 0.5 tone).
    fn speech(seconds: usize) -> Vec<f32> {
        vec![0.5; seconds * SAMPLE_RATE]
    }

    /// One second of digital silence.
    fn silence(seconds: usize) -> Vec<f32> {
        vec![0.0; seconds * SAMPLE_RATE]
    }

    /// Concatenates blocks of speech and silence into one buffer.
    fn take(blocks: &[Vec<f32>]) -> Vec<f32> {
        blocks.concat()
    }

    /// Drains every segment the segmenter can close over `buffer`.
    fn close_all(segmenter: &mut Segmenter, buffer: &[f32]) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        while let Some(range) = segmenter.poll(buffer) {
            ranges.push(range);
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
        let buffer = take(&[speech(2), silence(2)]);
        let mut segmenter = Segmenter::new();
        let ranges = close_all(&mut segmenter, &buffer);
        assert_eq!(ranges.len(), 1, "one speech run closes one segment");
        let range = &ranges[0];
        assert_eq!(range.start, 0);
        assert!(
            range.end <= 2 * SAMPLE_RATE + FRAME_SAMPLES,
            "the segment ends where the silence began: {range:?}"
        );
        assert!(
            range.end - range.start >= 2 * SAMPLE_RATE - FRAME_SAMPLES,
            "the segment holds the whole speech run: {range:?}"
        );
        assert_eq!(segmenter.consumed(), range.end);
    }

    #[test]
    fn a_short_pause_does_not_close_the_segment() {
        // Half a second of silence is inside the 700 ms closing threshold.
        let buffer = take(&[
            speech(1),
            silence(1).split_at(SAMPLE_RATE / 2).0.to_vec(),
            speech(1),
        ]);
        let mut segmenter = Segmenter::new();
        assert!(
            close_all(&mut segmenter, &buffer).is_empty(),
            "a sentence-internal pause must not split the segment"
        );
    }

    #[test]
    fn clicks_shorter_than_min_speech_are_discarded() {
        // 100 ms of tone followed by a full closing silence.
        let buffer = take(&[speech(1).split_at(SAMPLE_RATE / 10).0.to_vec(), silence(2)]);
        let mut segmenter = Segmenter::new();
        assert!(
            close_all(&mut segmenter, &buffer).is_empty(),
            "a 100 ms blip is a click, not a segment"
        );
        assert!(
            segmenter.consumed() > 0,
            "the click is still consumed so the tail excludes it"
        );
    }

    #[test]
    fn two_speech_runs_close_as_two_segments() {
        let buffer = take(&[speech(1), silence(1), speech(1), silence(1)]);
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
        assert!(segmenter.poll(&buffer).is_none());
        buffer.extend_from_slice(&silence(1));
        let first = segmenter.poll(&buffer).expect("the segment closes");
        assert_eq!(first.start, 0);
        // Polling again without new audio returns nothing.
        assert!(segmenter.poll(&buffer).is_none());
    }

    #[test]
    fn reset_rewinds_for_a_new_take() {
        let buffer = take(&[speech(1), silence(1)]);
        let mut segmenter = Segmenter::new();
        assert!(segmenter.poll(&buffer).is_some());
        segmenter.reset();
        assert_eq!(segmenter.consumed(), 0);
        assert!(
            segmenter.poll(&buffer).is_some(),
            "after reset the same buffer segments again"
        );
    }
}
