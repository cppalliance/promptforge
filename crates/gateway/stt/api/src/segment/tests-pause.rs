//! Tests for forced strides cut where speech resumed after a late pause.

use super::*;

fn forced(outcome: Option<SegmentOutcome>) -> ForcedBoundary {
    let Some(SegmentOutcome::Forced(boundary)) = outcome else {
        panic!("a forced boundary closes: {outcome:?}");
    };
    boundary
}

#[test]
fn a_96_ms_pause_9_2_s_into_continuous_speech_moves_the_cut_to_where_speech_resumed() {
    let (mut segmenter, _) = scripted(&[(0, 147_456), (148_992, 400_000)]);
    assert_eq!(
        hear(&mut segmenter, 160_255),
        None,
        "the cut is still decided 10.016 s after the onset"
    );
    let first = forced(hear(&mut segmenter, 160_256));
    assert_eq!(first.decode_range(), 0..148_992);
    assert_eq!(first.new_audio(), 0..148_992);
    assert_eq!(segmenter.consumed(), 148_992);
    assert_eq!(hear(&mut segmenter, 309_247), None);
    let second = forced(hear(&mut segmenter, 309_248));
    assert_eq!(
        second.new_audio(),
        148_992..309_248,
        "the successor's new audio starts where speech resumed"
    );
    assert_eq!(
        second.overlap(),
        Some(20_992..148_992),
        "the successor's overlap starts 8 s before the cut"
    );
}

#[test]
fn a_pause_starting_before_the_search_band_leaves_the_cut_at_the_stride() {
    let (mut segmenter, _) = scripted(&[(0, 135_680), (137_216, 400_000)]);
    assert_eq!(
        forced(hear(&mut segmenter, 160_256)).new_audio(),
        0..160_256,
        "a pause from the frame before the band, 8.48 s in, is ignored"
    );
}

#[test]
fn one_frame_gaps_leave_the_cut_at_the_stride() {
    let (mut segmenter, _) = scripted(&[(0, 150_016), (150_528, 155_136), (155_648, 400_000)]);
    assert_eq!(
        forced(hear(&mut segmenter, 160_256)).new_audio(),
        0..160_256
    );
}

#[test]
fn the_longer_of_two_pauses_in_the_band_wins_and_the_later_of_equal_ones() {
    let (mut longer_first, _) = scripted(&[(0, 140_288), (141_824, 150_016), (151_040, 400_000)]);
    assert_eq!(
        forced(hear(&mut longer_first, 160_256)).new_audio(),
        0..141_824,
        "a 96 ms pause beats a later 64 ms one"
    );
    let (mut equal, _) = scripted(&[(0, 140_288), (141_312, 150_016), (151_040, 400_000)]);
    assert_eq!(
        forced(hear(&mut equal, 160_256)).new_audio(),
        0..151_040,
        "of two 64 ms pauses the later wins"
    );
}

#[test]
fn the_successor_of_a_pause_cut_waits_out_a_short_breath_for_two_seconds() {
    let (mut segmenter, _) = scripted(&[(0, 147_456), (148_992, 160_768)]);
    assert_eq!(
        forced(hear(&mut segmenter, 160_256)).new_audio(),
        0..148_992
    );
    assert_eq!(
        hear(&mut segmenter, 164_352),
        None,
        "0.736 s of speech after the cut continues a longer run, so 0.2 s of silence is a breath"
    );
    assert_eq!(hear(&mut segmenter, 193_023), None);
    let successor = forced(hear(&mut segmenter, 193_024));
    assert_eq!(successor.overlap(), Some(20_992..148_992));
    assert_eq!(successor.new_audio(), 148_992..162_368);
    assert!(!successor.retains_overlap());
}

#[test]
fn every_chunk_is_classified_once_in_order_across_two_pause_cuts() {
    let (mut segmenter, detector) =
        scripted(&[(0, 147_456), (148_992, 296_448), (297_984, 340_000)]);
    let mut buffer = Vec::new();
    let mut strides = Vec::new();
    while buffer.len() < 336_000 {
        buffer.resize(buffer.len() + 1_000, 0.0);
        segmenter.classify(&buffer, 0, true).expect("classified");
        while let Some(outcome) = segmenter.poll() {
            strides.push(forced(Some(outcome)).new_audio());
        }
    }
    assert_eq!(strides, [0..148_992, 148_992..297_984]);
    let starts = (0..336_000 / FRAME_SAMPLES)
        .map(|chunk| chunk * FRAME_SAMPLES)
        .collect::<Vec<_>>();
    assert_eq!(detector.chunk_starts(), starts);
}
