//! Tests for where an interim window ends and starts: at the speech tail or
//! the buffer end, whichever comes first, and never before the open segment.

use std::ops::Range;

use gateway_stt_engine::FallbackDetector;
use gateway_stt_engine::test_fixtures::ScriptedDetector;

use super::SPEECH_TAIL_SAMPLES;
use crate::segment::FRAME_SAMPLES;
use crate::take::{Take, TakeState};

const TAIL: u64 = SPEECH_TAIL_SAMPLES;
const WIDE: usize = usize::MAX;

const fn at(frames: usize) -> u64 {
    (frames * FRAME_SAMPLES) as u64
}

/// A take without a final pass that has heard speech only in the `speech`
/// frames of `appended` frames of silent audio, so only the detector decides.
fn take_hearing(speech: Range<usize>, appended: usize) -> Take {
    let detector =
        ScriptedDetector::new([(speech.start * FRAME_SAMPLES, speech.end * FRAME_SAMPLES)]);
    let take = Take::with_detector(Vec::new(), None, FallbackDetector::new(Box::new(detector)));
    take.append(vec![0.0; appended * FRAME_SAMPLES])
        .expect("audio appends");
    take
}

fn open_segment_at(take: &Take, start: u64) {
    TakeState::lock(&take.state.segmenter).set_consumed_for_test(start);
}

/// The window's segment start, start, and end, after checking it holds
/// exactly the audio between them.
fn bounds(take: &Take, window_samples: usize) -> (u64, u64, u64) {
    let window = take
        .interim_window(window_samples)
        .expect("the window copies");
    assert_eq!(
        window.samples.len() as u64,
        window.end - window.start,
        "the window holds its own range"
    );
    (window.segment_start, window.start, window.end)
}

#[test]
fn a_window_ends_at_the_speech_tail_once_the_tail_has_arrived() {
    let take = take_hearing(0..20, 60);
    assert_eq!(bounds(&take, WIDE), (0, 0, at(20) + TAIL));
}

#[test]
fn a_window_ends_at_the_buffer_end_while_its_speech_tail_is_still_arriving() {
    let take = take_hearing(0..20, 22);
    assert_eq!(bounds(&take, WIDE), (0, 0, at(22)));
    let take = take_hearing(0..30, 30);
    assert_eq!(
        bounds(&take, WIDE),
        (0, 0, at(30)),
        "speech reaches the buffer end"
    );
}

#[test]
fn a_window_reaches_back_its_width_from_the_speech_tail_but_not_before_its_segment() {
    let take = take_hearing(0..40, 80);
    let end = at(40) + TAIL;
    assert_eq!(
        bounds(&take, 32 * FRAME_SAMPLES),
        (0, end - at(32), end),
        "the width is measured back from the speech tail, not the buffer end"
    );
    open_segment_at(&take, at(30));
    assert_eq!(bounds(&take, 32 * FRAME_SAMPLES), (at(30), at(30), end));
}

#[test]
fn a_segment_opened_after_the_speech_tail_has_an_empty_window_at_its_start() {
    let take = take_hearing(0..20, 80);
    open_segment_at(&take, at(60));
    assert_eq!(bounds(&take, WIDE), (at(60), at(60), at(60)));
}

#[test]
fn a_take_that_has_heard_no_speech_has_an_empty_window() {
    let take = take_hearing(0..0, 80);
    assert_eq!(bounds(&take, WIDE), (0, 0, 0));
}
