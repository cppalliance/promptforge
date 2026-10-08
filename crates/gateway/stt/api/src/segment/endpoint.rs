//! The pure endpoint rules that open and close speech segments.
//!
//! Three rules decide timing: silence with no tracked speech never opens a
//! segment, silence after speech closes it once the silence lasts
//! [`MIN_SILENCE_SAMPLES`] (or [`SENTENCE_END_SILENCE_SAMPLES`] when the take
//! hints that the speech ends a sentence or a speech run that does not
//! continue a stride is shorter than [`SHORT_BURST_SAMPLES`]), and speech
//! closes at the forced stride [`FORCED_STRIDE_SAMPLES`] after its onset, or
//! where it resumed after the longest pause in that stride's last
//! [`PAUSE_SEARCH_SAMPLES`]. Closing timing is
//! measured on the detector's speech run; the finalized segment adds a
//! pre-roll before the run and a hangover after it.

use std::ops::Range;

use gateway_stt_engine::EnginePolicy;

use super::{FORCED_OVERLAP_SAMPLES, FRAME_SAMPLES};

/// Silence must persist this long after speech to close a segment: 2 s,
/// long enough to survive sentence-internal pauses and natural breathing
/// gaps, short enough that the final pass starts well before the user stops
/// talking.
const MIN_SILENCE_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE * 2) as u64;

/// Silence that closes a segment whose latest accepted interim text ends in
/// sentence-final punctuation: 0.2 s, so a finished sentence reaches the final
/// pass without waiting out the pause allowance for a sentence still going.
const SENTENCE_END_SILENCE_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE / 5) as u64;

/// Speech runs shorter than this close after [`SENTENCE_END_SILENCE_SAMPLES`]
/// even without the sentence-end hint: 1 s, so a lone word or a short reply
/// reaches the final pass without waiting out the pause allowance for a
/// sentence still going. A run that continues through a stride is the tail
/// of a longer utterance, so it keeps that allowance however short it is.
pub(super) const SHORT_BURST_SAMPLES: u64 = EnginePolicy::SAMPLE_RATE as u64;

/// Speech runs this long before a stride closes it: 10 s rounded up to whole
/// frames (313 frames, 10.016 s), so a stride ends on the frame grid and
/// scanning after it stays on that grid.
const FORCED_STRIDE_SAMPLES: u64 =
    ((EnginePolicy::SAMPLE_RATE * 10).div_ceil(FRAME_SAMPLES) * FRAME_SAMPLES) as u64;

/// A pause starting this close to the stride end moves the stride's cut to
/// where speech resumed: 1.5 s rounded up to whole frames (47 frames,
/// 1.504 s), so the cut stays on the frame grid.
const PAUSE_SEARCH_SAMPLES: u64 =
    ((EnginePolicy::SAMPLE_RATE * 3 / 2).div_ceil(FRAME_SAMPLES) * FRAME_SAMPLES) as u64;

/// Non-speech shorter than two frames (64 ms) is a word gap, not a pause.
const MIN_PAUSE_SAMPLES: u64 = 2 * FRAME_SAMPLES as u64;

/// Closing silence kept at the end of a segment: 100 ms, so a trailing
/// consonant the detector reads as silence still reaches the final pass.
pub(crate) const HANGOVER_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE / 10) as u64;

/// Audio kept before a segment's first speech frame: 0.5 s, so a soft onset
/// the detector misses still reaches the final pass.
const PRE_ROLL_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE / 2) as u64;

const _: () = assert!(HANGOVER_SAMPLES < SENTENCE_END_SILENCE_SAMPLES);
const _: () = assert!(SENTENCE_END_SILENCE_SAMPLES < MIN_SILENCE_SAMPLES);
// A stride cut at its earliest pause still holds its successor's overlap.
const _: () = assert!(FORCED_STRIDE_SAMPLES - PAUSE_SEARCH_SAMPLES > FORCED_OVERLAP_SAMPLES as u64);

/// Segmenter state the endpoint rules read and advance.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct EndpointState {
    /// First sample of the speech run being tracked, if any.
    pub(super) speech_start: Option<u64>,
    /// First sample of the silence after the tracked speech, if one began.
    pub(super) silence_begin: Option<u64>,
    /// End of the last completed segment: everything before this index has
    /// been handed to the final pass (or discarded as a click).
    pub(super) consumed: u64,
    /// Whether the tracked speech run began where a stride cut speech that
    /// was still going.
    pub(super) continues_stride: bool,
    /// The tracked run's longest pause in its stride's search band, the
    /// latest of equal ones.
    pause: Option<Pause>,
}

/// Non-speech inside a speech run, ended where speech resumed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Pause {
    begin: u64,
    resumed: u64,
}

impl EndpointState {
    /// Where a segment whose speech begins at `onset` starts.
    fn pre_rolled(self, onset: u64) -> u64 {
        onset.saturating_sub(PRE_ROLL_SAMPLES).max(self.consumed)
    }

    /// The cut candidate once speech resumes at `resumed` in the run that
    /// began at `onset`.
    fn pause_resumed(self, onset: u64, resumed: u64) -> Option<Pause> {
        let band = onset.saturating_add(FORCED_STRIDE_SAMPLES - PAUSE_SEARCH_SAMPLES);
        let needed = self
            .pause
            .map_or(MIN_PAUSE_SAMPLES, |best| best.resumed - best.begin);
        match self.silence_begin {
            Some(begin) if begin >= band && resumed - begin >= needed => {
                Some(Pause { begin, resumed })
            }
            _ => self.pause,
        }
    }
}

/// The scan position the rules decide at.
#[derive(Clone, Copy, Debug)]
pub(super) struct Scan {
    /// First sample of the next analysis frame.
    pub(super) cursor: u64,
    /// End of the audio classified so far.
    pub(super) received: u64,
    /// Whether the detector reads the next frame as silence, or `None`
    /// until that whole frame is classified.
    pub(super) silent: Option<bool>,
    /// Whether the take hints that the tracked speech ends a sentence.
    pub(super) sentence_end: bool,
}

/// Which rule closed a segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Rule {
    Silence,
    Stride,
}

/// A segment one of the closing rules ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Closed {
    pub(super) rule: Rule,
    /// The speech run the detector tracked, from its first speech frame.
    pub(super) speech: Range<u64>,
    /// The audio to finalize: the speech run with its pre-roll and hangover.
    pub(super) segment: Range<u64>,
}

/// One decision of the rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Advance {
    pub(super) state: EndpointState,
    /// The next unscanned sample.
    pub(super) cursor: u64,
    pub(super) closed: Option<Closed>,
}

/// Applies the endpoint rules at `scan`, or returns `None` while the next
/// decision needs audio that has not arrived.
#[must_use]
pub(super) fn endpoint(state: EndpointState, scan: Scan) -> Option<Advance> {
    let frame_end = scan.cursor.saturating_add(FRAME_SAMPLES as u64);
    if let Some(onset) = state.speech_start {
        let stride_end = onset.checked_add(FORCED_STRIDE_SAMPLES)?;
        if stride_end <= scan.received && frame_end > stride_end {
            let (cut, continues) = match state.pause {
                Some(pause) => (pause.resumed, true),
                None => (stride_end, state.silence_begin.is_none()),
            };
            return Some(Advance {
                state: EndpointState {
                    speech_start: continues.then_some(cut),
                    silence_begin: state.silence_begin.filter(|_| continues),
                    consumed: cut,
                    continues_stride: continues,
                    pause: None,
                },
                cursor: stride_end,
                closed: Some(Closed {
                    rule: Rule::Stride,
                    speech: onset..cut,
                    segment: state.pre_rolled(onset)..cut,
                }),
            });
        }
    }
    let silent = scan.silent?;
    let mut next = state;
    let closed = match (state.speech_start, silent) {
        (None, true) => None,
        (None, false) => {
            next.speech_start = Some(scan.cursor);
            None
        }
        (Some(onset), false) => {
            next.pause = state.pause_resumed(onset, scan.cursor);
            next.silence_begin = None;
            None
        }
        (Some(onset), true) => {
            let begin = *next.silence_begin.get_or_insert(scan.cursor);
            let short_burst = !state.continues_stride && begin - onset < SHORT_BURST_SAMPLES;
            let closing = if scan.sentence_end || short_burst {
                SENTENCE_END_SILENCE_SAMPLES
            } else {
                MIN_SILENCE_SAMPLES
            };
            (frame_end - begin >= closing).then(|| {
                let end = begin + HANGOVER_SAMPLES;
                next = EndpointState {
                    speech_start: None,
                    silence_begin: None,
                    consumed: end,
                    continues_stride: false,
                    pause: None,
                };
                Closed {
                    rule: Rule::Silence,
                    speech: onset..begin,
                    segment: state.pre_rolled(onset)..end,
                }
            })
        }
    };
    Some(Advance {
        state: next,
        cursor: frame_end,
        closed,
    })
}

#[cfg(test)]
#[path = "endpoint-tests.rs"]
mod tests;
