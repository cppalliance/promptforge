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
        pause: None,
    }
}

/// Speech from sample 0 whose cut candidate is the pause from `begin` to
/// where speech `resumed`.
fn paused(begin: u64, resumed: u64, silence_begin: Option<u64>) -> EndpointState {
    EndpointState {
        pause: Some(Pause { begin, resumed }),
        ..speaking(0, silence_begin, 0)
    }
}

fn closed(advance: Option<Advance>) -> Closed {
    advance
        .and_then(|advance| advance.closed)
        .expect("the rule closes a segment")
}

fn candidate(advance: Option<Advance>) -> Option<Pause> {
    advance.expect("a whole frame decides").state.pause
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
    let advance =
        endpoint(speaking(0, Some(158_208), 0), scan(160_256, true)).expect("the stride closes");
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

#[test]
fn speech_resuming_after_two_silent_frames_in_the_search_band_marks_a_cut_candidate() {
    let advance = endpoint(speaking(0, Some(136_192), 0), scan(137_216, false));
    assert_eq!(
        candidate(advance),
        Some(Pause {
            begin: 136_192,
            resumed: 137_216
        }),
        "the band opens 266 frames, 8.512 s, after the onset"
    );
}

#[test]
fn a_pause_starting_before_the_search_band_is_no_candidate() {
    let advance = endpoint(speaking(0, Some(135_680), 0), scan(137_216, false));
    assert_eq!(
        candidate(advance),
        None,
        "a pause reaching into the band from the frame before it is ignored"
    );
}

#[test]
fn a_one_frame_gap_is_no_candidate() {
    let advance = endpoint(speaking(0, Some(150_016), 0), scan(150_016 + FRAME, false));
    assert_eq!(candidate(advance), None);
}

#[test]
fn the_longest_pause_is_the_candidate_and_the_latest_of_equal_ones() {
    let three_frames = paused(140_288, 141_824, Some(150_016));
    assert_eq!(
        candidate(endpoint(three_frames, scan(151_040, false))),
        Some(Pause {
            begin: 140_288,
            resumed: 141_824
        }),
        "a later two-frame pause loses to an earlier three-frame one"
    );
    assert_eq!(
        candidate(endpoint(three_frames, scan(151_552, false))),
        Some(Pause {
            begin: 150_016,
            resumed: 151_552
        }),
        "of two three-frame pauses the later wins"
    );
    assert_eq!(
        candidate(endpoint(three_frames, scan(152_064, false))),
        Some(Pause {
            begin: 150_016,
            resumed: 152_064
        }),
        "a longer pause wins"
    );
}

#[test]
fn a_stride_with_a_candidate_cuts_where_speech_resumed() {
    let advance =
        endpoint(paused(147_456, 148_992, None), scan(160_256, false)).expect("the stride closes");
    assert_eq!(
        advance.cursor, 160_256,
        "scanning goes on from the stride end"
    );
    assert_eq!(
        advance.state,
        EndpointState {
            continues_stride: true,
            ..speaking(148_992, None, 148_992)
        },
        "the next run starts where speech resumed and continues the stride"
    );
    let stride = advance.closed.expect("the stride closes a segment");
    assert_eq!(stride.rule, Rule::Stride);
    assert_eq!(stride.speech, 0..148_992);
    assert_eq!(stride.segment, 0..148_992);
}

#[test]
fn a_stride_during_a_pause_after_a_candidate_tracks_the_speech_since_the_cut() {
    let advance = endpoint(paused(147_456, 148_992, Some(155_648)), scan(160_256, true));
    assert_eq!(
        advance.as_ref().map(|advance| advance.state),
        Some(EndpointState {
            continues_stride: true,
            ..speaking(148_992, Some(155_648), 148_992)
        }),
        "the silence that began after the cut goes on closing the next run"
    );
    assert_eq!(closed(advance).speech, 0..148_992);
}

#[test]
fn a_silence_close_clears_the_candidate() {
    let hinted = Scan {
        sentence_end: true,
        ..scan(150_016 + SENTENCE_END - FRAME, true)
    };
    let advance = endpoint(paused(140_288, 141_824, Some(150_016)), hinted);
    assert_eq!(
        advance.as_ref().map(|advance| advance.state),
        Some(EndpointState {
            consumed: 150_016 + HANGOVER_SAMPLES,
            ..EndpointState::default()
        })
    );
    assert_eq!(closed(advance).speech, 0..150_016);
}
