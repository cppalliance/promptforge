//! Voice activity segmentation for the pipelined final pass.
//!
//! [`Segmenter`] classifies a growing take buffer one detector chunk at a
//! time and reports a completed speech segment each time a run of silence
//! long enough to be a segment boundary follows speech. The session hands
//! each reported range to the final-pass worker while the take is still
//! recording, so on `stop` only the unclosed tail remains to transcribe.

use std::collections::VecDeque;
use std::ops::Range;

use gateway_stt_engine::{DetectorError, EnginePolicy, FallbackDetector};

mod boundary;
mod endpoint;

pub(crate) use boundary::{ForcedBoundary, SegmentOutcome};
use endpoint::{Closed, EndpointState, HANGOVER_SAMPLES, Rule, Scan};

/// Analysis frame length: one detector chunk, 32 ms at 16 kHz.
pub(crate) const FRAME_SAMPLES: usize = EnginePolicy::DETECTOR_CHUNK_SAMPLES;
pub(crate) const FORCED_OVERLAP_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * 8;

/// Speech shorter than 250 ms is discarded as a click or cough rather than
/// transcribed, where whisper would hallucinate a word for it.
const MIN_SPEECH_SAMPLES: usize = EnginePolicy::SAMPLE_RATE / 4;

/// Incremental speech segmenter over one take's PCM buffer.
///
/// The buffer is append-only for the life of a take. Each
/// [`classify`](Segmenter::classify) hands the frames completed since the
/// last call to the take's detector, on one grid from sample 0, and each
/// [`poll`](Segmenter::poll) applies the endpoint rules to the decisions
/// classified since the last poll. Ranges are indices into that buffer.
#[derive(Debug)]
pub(crate) struct Segmenter {
    detector: FallbackDetector,
    /// End of the latest classified frame.
    classified: u64,
    /// The speech runs classified at or after `cursor`, oldest first.
    queued: VecDeque<Range<u64>>,
    /// Next frame the endpoint rules have not decided on.
    cursor: u64,
    /// The tracked speech run and the end of the last completed segment.
    endpoint: EndpointState,
    /// End of the preceding forced stride when uninterrupted speech may
    /// overlap it.
    forced_predecessor: Option<u64>,
    /// Start of the segment whose latest accepted interim text ends a
    /// sentence, if one does.
    sentence_end: Option<u64>,
    /// End of the latest classified frame the detector read as speech.
    speech_end: u64,
    /// Every speech run classified, oldest first.
    #[cfg(any(test, feature = "test-fixtures"))]
    speech_runs: Vec<Range<u64>>,
}

/// What the detector heard before some sample: where its last speech frame
/// ended, with speech classified after that sample counting as reaching it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpeechBefore {
    speech_end: u64,
}

impl SpeechBefore {
    #[cfg(test)]
    pub(crate) const fn for_test(speech_end: u64) -> Self {
        Self { speech_end }
    }

    /// An upper bound on the speech heard after `since`, past the hangover
    /// that the trailing sound of a word ending at `since` may spill over.
    pub(crate) const fn after(self, since: u64) -> u64 {
        self.speech_end
            .saturating_sub(since.saturating_add(HANGOVER_SAMPLES))
    }
}

impl Segmenter {
    /// A fresh segmenter positioned at the start of a take buffer.
    #[must_use]
    pub(crate) fn new(detector: FallbackDetector) -> Self {
        Self {
            detector,
            classified: 0,
            queued: VecDeque::new(),
            cursor: 0,
            endpoint: EndpointState::default(),
            forced_predecessor: None,
            sentence_end: None,
            speech_end: 0,
            #[cfg(any(test, feature = "test-fixtures"))]
            speech_runs: Vec::new(),
        }
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

    /// The next frame the endpoint rules have not decided on. Right after a
    /// silence rule closes a segment, the audio from the segment's end
    /// through it is silence.
    pub(crate) const fn scanned(&self) -> u64 {
        self.cursor
    }

    /// End of the latest frame the detector read as speech, or `None`
    /// before it has read any.
    pub(crate) fn speech_end(&self) -> Option<u64> {
        (self.speech_end > 0).then_some(self.speech_end)
    }

    /// What the detector heard before `end`, or `None` while a whole frame
    /// before `end` is unclassified.
    pub(crate) fn speech_before(&self, end: u64) -> Option<SpeechBefore> {
        (self.classified.saturating_add(FRAME_SAMPLES as u64) >= end).then(|| SpeechBefore {
            speech_end: self.speech_end.min(end),
        })
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

    /// Classifies every frame completed since the last call, in order, and
    /// queues the decisions for [`poll`](Self::poll) when `closes_segments`.
    /// A take without a final pipeline never polls, so it queues nothing.
    pub(crate) fn classify(&mut self, buffer: &[f32], buffer_origin: u64, closes_segments: bool) {
        debug_assert!(self.classified >= buffer_origin);
        while let Some(frame) = self
            .classified
            .checked_sub(buffer_origin)
            .and_then(|start| usize::try_from(start).ok())
            .and_then(|start| buffer.get(start..start.checked_add(FRAME_SAMPLES)?))
        {
            let start = self.classified;
            self.classified += FRAME_SAMPLES as u64;
            if !self.detector.classify(frame) {
                continue;
            }
            self.speech_end = self.classified;
            if closes_segments {
                match self.queued.back_mut() {
                    Some(run) if run.end == start => run.end = self.classified,
                    _ => self.queued.push_back(start..self.classified),
                }
            }
            #[cfg(any(test, feature = "test-fixtures"))]
            match self.speech_runs.last_mut() {
                Some(run) if run.end == start => run.end = self.classified,
                _ => self.speech_runs.push(start..self.classified),
            }
        }
    }

    /// Hands out the detector's first failure once, for reporting.
    pub(crate) fn take_fault(&mut self) -> Option<DetectorError> {
        self.detector.take_fault()
    }

    /// Every speech run classified so far, on the frame grid from sample 0.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub(crate) fn speech_runs(&self) -> &[Range<u64>] {
        &self.speech_runs
    }

    /// Applies the endpoint rules to the queued decisions and returns the
    /// range of the next completed speech segment, if one closed. Call in a
    /// loop: a large arrival can complete more than one segment.
    pub(crate) fn poll(&mut self) -> Option<SegmentOutcome> {
        loop {
            while self
                .queued
                .front()
                .is_some_and(|run| run.end <= self.cursor)
            {
                self.queued.pop_front();
            }
            let frame_end = self.cursor.saturating_add(FRAME_SAMPLES as u64);
            let silent = (frame_end <= self.classified).then(|| {
                self.queued
                    .front()
                    .is_none_or(|run| run.start > self.cursor)
            });
            let scan = Scan {
                cursor: self.cursor,
                received: self.classified,
                silent,
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

    /// Classifies a closed segment. Overlap ownership and the click rule read
    /// the speech run, so padding never turns a click into a decode, or a
    /// resumed run into a successor of the stride before its pause. Any other
    /// run holds a word, so the final pass decodes it, however short.
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
            Rule::Silence => SegmentOutcome::Decode(self.reaching_final_window(segment)),
        }
    }

    /// `segment`, extended into the closing silence already scanned until it
    /// holds the final window, because the final pass skips a shorter window.
    /// Only a run whose pre-roll the take start or a stride cut short needs it.
    fn reaching_final_window(&mut self, segment: Range<u64>) -> Range<u64> {
        let floor = segment
            .start
            .saturating_add(EnginePolicy::MIN_WINDOW_SAMPLES as u64);
        let end = segment.end.max(floor.min(self.cursor));
        self.endpoint.consumed = end;
        segment.start..end
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
