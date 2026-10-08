//! The pure endpoint rules that open and close speech segments.
//!
//! Three rules decide timing: silence with no tracked speech never opens a
//! segment, silence after speech closes it once the silence lasts
//! [`MIN_SILENCE_SAMPLES`] (or [`SENTENCE_END_SILENCE_SAMPLES`] when the take
//! hints that the speech ends a sentence or a speech run that does not
//! continue a stride is shorter than [`SHORT_BURST_SAMPLES`]), and speech
//! closes at the forced stride [`FORCED_STRIDE_SAMPLES`] after its onset.
//! Closing timing is
//! measured on the detector's speech run; the finalized segment adds a
//! pre-roll before the run and a hangover after it.

use std::ops::Range;

use gateway_stt_engine::EnginePolicy;

use super::FRAME_SAMPLES;

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

/// Closing silence kept at the end of a segment: 100 ms, so a trailing
/// consonant the detector reads as silence still reaches the final pass.
pub(crate) const HANGOVER_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE / 10) as u64;

/// Audio kept before a segment's first speech frame: 0.5 s, so a soft onset
/// the detector misses still reaches the final pass.
const PRE_ROLL_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE / 2) as u64;

const _: () = assert!(HANGOVER_SAMPLES < SENTENCE_END_SILENCE_SAMPLES);
const _: () = assert!(SENTENCE_END_SILENCE_SAMPLES < MIN_SILENCE_SAMPLES);

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
}

impl EndpointState {
    /// Where a segment whose speech begins at `onset` starts.
    fn pre_rolled(self, onset: u64) -> u64 {
        onset.saturating_sub(PRE_ROLL_SAMPLES).max(self.consumed)
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
            let continues = state.silence_begin.is_none();
            return Some(Advance {
                state: EndpointState {
                    speech_start: continues.then_some(stride_end),
                    silence_begin: None,
                    consumed: stride_end,
                    continues_stride: continues,
                },
                cursor: stride_end,
                closed: Some(Closed {
                    rule: Rule::Stride,
                    speech: onset..stride_end,
                    segment: state.pre_rolled(onset)..stride_end,
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
        (Some(_), false) => {
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
mod tests {
    use super::*;

    const FRAME: u64 = FRAME_SAMPLES as u64;
    const SENTENCE_END: u64 = SENTENCE_END_SILENCE_SAMPLES;

    fn scan(cursor: u64, silent: bool) -> Scan {
        Scan {
            cursor,
            received: cursor + FRAME,
            silent: Some(silent),
            sentence_end: false,
        }
    }

    fn speaking(onset: u64, silence_begin: Option<u64>, consumed: u64) -> EndpointState {
        EndpointState {
            speech_start: Some(onset),
            silence_begin,
            consumed,
            continues_stride: false,
        }
    }

    fn closed(advance: Option<Advance>) -> Closed {
        advance
            .and_then(|advance| advance.closed)
            .expect("the rule closes a segment")
    }

    #[test]
    fn silence_with_no_speech_never_opens_a_segment() {
        let idle = EndpointState {
            consumed: 4_800,
            ..EndpointState::default()
        };
        assert_eq!(
            endpoint(idle, scan(96_000, true)),
            Some(Advance {
                state: idle,
                cursor: 96_000 + FRAME,
                closed: None,
            })
        );
    }

    #[test]
    fn a_speech_frame_opens_a_run_at_its_first_sample() {
        let advance = endpoint(EndpointState::default(), scan(9_600, false));
        assert_eq!(
            advance.map(|advance| advance.state.speech_start),
            Some(Some(9_600))
        );
    }

    #[test]
    fn silence_after_speech_closes_once_a_frame_reaches_two_seconds() {
        let state = speaking(0, Some(38_400), 0);
        let short = endpoint(state, scan(38_400 + 32_000 - FRAME - 1, true));
        assert_eq!(short.and_then(|advance| advance.closed), None);
        let closing = closed(endpoint(state, scan(38_400 + 32_000 - FRAME, true)));
        assert_eq!(closing.rule, Rule::Silence);
        assert_eq!(closing.speech, 0..38_400);
    }

    #[test]
    fn a_sentence_end_hint_closes_once_a_frame_reaches_two_tenths_of_a_second() {
        let state = speaking(0, Some(38_400), 0);
        let hinted = |cursor| Scan {
            sentence_end: true,
            ..scan(cursor, true)
        };
        let short = endpoint(state, hinted(38_400 + SENTENCE_END - FRAME - 1));
        assert_eq!(short.and_then(|advance| advance.closed), None);
        let closing = closed(endpoint(state, hinted(38_400 + SENTENCE_END - FRAME)));
        assert_eq!(closing.rule, Rule::Silence);
        assert_eq!(closing.speech, 0..38_400);
        assert_eq!(closing.segment, 0..40_000);
        let unhinted = endpoint(state, scan(38_400 + SENTENCE_END - FRAME, true));
        assert_eq!(
            unhinted.and_then(|advance| advance.closed),
            None,
            "without the hint the same silence waits for two seconds"
        );
    }

    #[test]
    fn a_burst_under_one_second_closes_once_a_frame_reaches_two_tenths_of_a_second() {
        let state = speaking(0, Some(15_999), 0);
        let short = endpoint(state, scan(15_999 + SENTENCE_END - FRAME - 1, true));
        assert_eq!(short.and_then(|advance| advance.closed), None);
        let closing = closed(endpoint(state, scan(15_999 + SENTENCE_END - FRAME, true)));
        assert_eq!(closing.rule, Rule::Silence);
        assert_eq!(closing.speech, 0..15_999);
        assert_eq!(closing.segment, 0..17_599);
    }

    #[test]
    fn a_burst_of_one_second_waits_for_two_seconds_of_silence() {
        let state = speaking(0, Some(16_000), 0);
        let paused = endpoint(state, scan(16_000 + SENTENCE_END - FRAME, true));
        assert_eq!(
            paused.and_then(|advance| advance.closed),
            None,
            "a run at the limit is not a short burst"
        );
        let short = endpoint(state, scan(16_000 + 32_000 - FRAME - 1, true));
        assert_eq!(short.and_then(|advance| advance.closed), None);
        let closing = closed(endpoint(state, scan(16_000 + 32_000 - FRAME, true)));
        assert_eq!(closing.speech, 0..16_000);
    }

    #[test]
    fn a_sentence_end_hint_closes_bursts_on_both_sides_of_the_limit_at_two_tenths_of_a_second() {
        for run in [15_999, 16_000] {
            let state = speaking(0, Some(run), 0);
            let hinted = |cursor| Scan {
                sentence_end: true,
                ..scan(cursor, true)
            };
            let short = endpoint(state, hinted(run + SENTENCE_END - FRAME - 1));
            assert_eq!(short.and_then(|advance| advance.closed), None);
            let closing = closed(endpoint(state, hinted(run + SENTENCE_END - FRAME)));
            assert_eq!(closing.speech, 0..run);
        }
    }

    #[test]
    fn speech_closes_at_the_forced_stride_once_its_audio_arrives() {
        let state = speaking(1_024, None, 0);
        let waiting = Scan {
            cursor: 161_280,
            received: 161_279,
            silent: None,
            sentence_end: false,
        };
        assert_eq!(endpoint(state, waiting), None);
        let advance = endpoint(
            state,
            Scan {
                received: 161_280,
                ..waiting
            },
        )
        .expect("the stride closes");
        assert_eq!(advance.cursor, 161_280);
        assert_eq!(
            advance.state,
            EndpointState {
                continues_stride: true,
                ..speaking(161_280, None, 161_280)
            },
            "speech continuing through the stride opens the next run at its end"
        );
        let stride = advance.closed.expect("the stride closes a segment");
        assert_eq!(stride.rule, Rule::Stride);
        assert_eq!(
            stride.speech,
            1_024..161_280,
            "the stride is 313 whole frames, 10.016 s"
        );
    }

    #[test]
    fn a_short_pause_right_after_a_stride_waits_for_two_seconds_of_silence() {
        let stride = endpoint(speaking(0, None, 0), scan(160_256, false))
            .expect("the stride closes")
            .state;
        let paused = EndpointState {
            silence_begin: Some(168_448),
            ..stride
        };
        let breath = endpoint(paused, scan(168_448 + SENTENCE_END - FRAME, true));
        assert_eq!(
            breath.and_then(|advance| advance.closed),
            None,
            "half a second of speech past the stride continues a longer run"
        );
        let short = endpoint(paused, scan(168_448 + 32_000 - FRAME - 1, true));
        assert_eq!(short.and_then(|advance| advance.closed), None);
        let advance = endpoint(paused, scan(168_448 + 32_000 - FRAME, true));
        assert_eq!(
            advance.as_ref().map(|advance| advance.state),
            Some(EndpointState {
                consumed: 168_448 + HANGOVER_SAMPLES,
                ..EndpointState::default()
            }),
            "the next run starts as a fresh burst"
        );
        let closing = closed(advance);
        assert_eq!(closing.speech, 160_256..168_448);
        let hinted = Scan {
            sentence_end: true,
            ..scan(168_448 + SENTENCE_END - FRAME, true)
        };
        assert_eq!(
            closed(endpoint(paused, hinted)).speech,
            160_256..168_448,
            "a sentence-end hint still closes at two tenths of a second"
        );
    }

    #[test]
    fn a_stride_during_a_pause_tracks_no_speech_after_it() {
        let advance = endpoint(speaking(0, Some(158_208), 0), scan(160_256, true))
            .expect("the stride closes");
        assert_eq!(
            advance.state,
            EndpointState {
                consumed: 160_256,
                ..EndpointState::default()
            }
        );
    }

    #[test]
    fn hangover_keeps_100_ms_of_the_closing_silence() {
        let advance = endpoint(
            speaking(0, Some(38_400), 0),
            scan(38_400 + 32_000 - FRAME, true),
        )
        .expect("a whole frame decides");
        assert_eq!(
            advance.state.consumed, 40_000,
            "the next segment starts no earlier than the hangover's end"
        );
        let closing = closed(Some(advance));
        assert_eq!(closing.speech.end, 38_400);
        assert_eq!(closing.segment.end, 40_000);
    }

    #[test]
    fn pre_roll_starts_a_segment_half_a_second_before_its_speech() {
        let closing = closed(endpoint(
            speaking(24_000, Some(62_400), 0),
            scan(62_400 + 32_000 - FRAME, true),
        ));
        assert_eq!(closing.segment.start, 16_000);
        let stride = closed(endpoint(speaking(24_064, None, 0), scan(184_320, false)));
        assert_eq!(stride.segment, 16_064..184_320);
    }

    #[test]
    fn pre_roll_never_reaches_before_the_consumed_cursor() {
        let closing = closed(endpoint(
            speaking(24_000, Some(62_400), 20_000),
            scan(62_400 + 32_000 - FRAME, true),
        ));
        assert_eq!(closing.segment.start, 20_000);
        let early = closed(endpoint(
            speaking(2_880, Some(41_280), 0),
            scan(41_280 + 32_000 - FRAME, true),
        ));
        assert_eq!(early.segment.start, 0);
    }
}
