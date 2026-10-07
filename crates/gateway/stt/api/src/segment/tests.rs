use super::*;

/// One second of loud synthetic speech (a constant 0.5 tone).
fn speech(seconds: usize) -> Vec<f32> {
    vec![0.5; seconds * EnginePolicy::SAMPLE_RATE]
}

/// One second of digital silence.
fn silence(seconds: usize) -> Vec<f32> {
    vec![0.0; seconds * EnginePolicy::SAMPLE_RATE]
}

/// Loud synthetic speech or digital silence, `samples` long.
fn run(loud: bool, samples: usize) -> Vec<f32> {
    vec![if loud { 0.5 } else { 0.0 }; samples]
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
        range.end
            <= (2 * EnginePolicy::SAMPLE_RATE + FRAME_SAMPLES + EnginePolicy::SAMPLE_RATE / 10)
                as u64,
        "the segment ends 100 ms past where the silence began: {range:?}"
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
        SegmentOutcome::Skipped(
            0..(EnginePolicy::SAMPLE_RATE * 3 / 25 + EnginePolicy::SAMPLE_RATE / 10) as u64
        ),
        "the frame-aligned click coverage and its hangover are retained for reconciliation"
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
fn first_natural_boundary_keeps_then_resets_forced_overlap_ownership() {
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
        panic!("the forced successor holds reconciliation metadata");
    };
    assert_eq!(natural.overlap(), Some(32_000..160_000));
    assert_eq!(natural.new_audio().start, 160_000);
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

#[test]
fn natural_segment_ends_a_hangover_past_the_silence_and_closes_after_two_seconds() {
    let buffer = take(&[run(true, 38_400), run(false, 48_000)]);
    let mut segmenter = Segmenter::new();
    assert_eq!(
        segmenter.poll(&buffer[..70_559], 0),
        None,
        "silence from 38,400 closes only once a frame reaches two seconds past it"
    );
    assert_eq!(
        segmenter.poll(&buffer[..70_560], 0),
        Some(SegmentOutcome::Decode(0..40_000)),
        "the segment keeps 100 ms of the closing silence"
    );
    assert_eq!(segmenter.consumed(), 40_000);
}

#[test]
fn natural_segment_starts_half_a_second_before_its_speech() {
    let buffer = take(&[run(false, 24_000), run(true, 38_400), run(false, 48_000)]);
    let mut segmenter = Segmenter::new();
    assert_eq!(
        segmenter.poll(&buffer, 0),
        Some(SegmentOutcome::Decode(16_000..64_000))
    );
}

#[test]
fn pre_roll_never_reaches_before_the_start_of_the_take() {
    let buffer = take(&[run(false, 2_880), run(true, 38_400), run(false, 48_000)]);
    let mut segmenter = Segmenter::new();
    assert_eq!(
        segmenter.poll(&buffer, 0),
        Some(SegmentOutcome::Decode(0..42_880))
    );
}

#[test]
fn pre_roll_after_a_paused_stride_stops_at_the_stride_and_starts_a_fresh_segment() {
    let buffer = take(&[
        run(true, 158_400),
        run(false, 2_080),
        run(true, 38_400),
        run(false, 48_000),
    ]);
    let mut segmenter = Segmenter::new();
    let Some(SegmentOutcome::Forced(stride)) = segmenter.poll(&buffer, 0) else {
        panic!("ten seconds after the onset force a stride");
    };
    assert_eq!(stride.decode_range(), 0..160_000);
    assert_eq!(
        segmenter.poll(&buffer, 0),
        Some(SegmentOutcome::Decode(160_000..200_480)),
        "speech resumed after a pause does not overlap the stride it follows"
    );
}

#[test]
fn forced_stride_keeps_its_ten_second_timing_and_gains_the_pre_roll() {
    let buffer = take(&[run(false, 2_880), run(true, 320_000)]);
    let mut segmenter = Segmenter::new();
    assert_eq!(segmenter.poll(&buffer[..162_879], 0), None);
    let Some(SegmentOutcome::Forced(first)) = segmenter.poll(&buffer, 0) else {
        panic!("ten seconds after the onset force a stride");
    };
    assert_eq!(first.decode_range(), 0..162_880);
    assert_eq!(first.new_audio(), 0..162_880);
    let Some(SegmentOutcome::Forced(second)) = segmenter.poll(&buffer, 0) else {
        panic!("continuous speech forces the next stride");
    };
    assert_eq!(second.overlap(), Some(34_880..162_880));
    assert_eq!(second.new_audio(), 162_880..322_880);
}

#[test]
fn click_length_is_measured_without_pre_roll_or_hangover() {
    let buffer = take(&[run(false, 24_000), run(true, 3_840), run(false, 48_000)]);
    let mut segmenter = Segmenter::new();
    assert_eq!(
        segmenter.poll(&buffer, 0),
        Some(SegmentOutcome::Skipped(16_000..29_440)),
        "a 240 ms run is a click even though its padded segment is longer"
    );
    assert_eq!(segmenter.consumed(), 29_440);
}
