use gateway_stt_engine::EnergyDetector;
use gateway_stt_engine::test_fixtures::ScriptedDetector;

use super::*;

const FRAME: u64 = FRAME_SAMPLES as u64;

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

/// A segmenter that reads speech by loudness.
fn energy() -> Segmenter {
    Segmenter::new(Box::new(EnergyDetector))
}

/// A segmenter whose detector hears speech only in the half-open sample
/// `runs`, whatever the audio holds, and a clone of that detector.
fn scripted(runs: &[(usize, usize)]) -> (Segmenter, ScriptedDetector) {
    let detector = ScriptedDetector::new(runs.iter().copied());
    let segmenter = Segmenter::new(Box::new(detector.clone()));
    (segmenter, detector)
}

/// Classifies `buffer`, which starts at sample 0, and closes the next segment.
fn poll(segmenter: &mut Segmenter, buffer: &[f32]) -> Option<SegmentOutcome> {
    segmenter.classify(buffer, 0, true).expect("classified");
    segmenter.poll()
}

/// Classifies audio through sample `end` and closes the next segment.
fn hear(segmenter: &mut Segmenter, end: usize) -> Option<SegmentOutcome> {
    poll(segmenter, &vec![0.0; end])
}

/// Drains every segment the segmenter can close over `buffer`.
fn close_all(segmenter: &mut Segmenter, buffer: &[f32]) -> Vec<Range<u64>> {
    segmenter.classify(buffer, 0, true).expect("classified");
    let mut ranges = Vec::new();
    while let Some(outcome) = segmenter.poll() {
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
    let mut segmenter = energy();
    assert!(close_all(&mut segmenter, &buffer).is_empty());
    assert_eq!(segmenter.consumed(), 0);
}

#[test]
fn ongoing_speech_does_not_close() {
    let buffer = speech(5);
    let mut segmenter = energy();
    assert!(
        close_all(&mut segmenter, &buffer).is_empty(),
        "a segment closes only on trailing silence"
    );
    assert_eq!(segmenter.consumed(), 0);
}

#[test]
fn speech_closes_after_enough_silence() {
    let buffer = take(&[speech(2), silence(3)]);
    let mut segmenter = energy();
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
    let mut segmenter = energy();
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
    let mut segmenter = energy();
    let outcome =
        poll(&mut segmenter, &buffer).expect("the discarded click is an explicit outcome");
    assert_eq!(
        outcome,
        SegmentOutcome::Skipped(0..(4 * FRAME_SAMPLES + EnginePolicy::SAMPLE_RATE / 10) as u64),
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
    let mut segmenter = energy();
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
    let mut segmenter = energy();
    assert!(poll(&mut segmenter, &buffer).is_none());
    buffer.extend_from_slice(&silence(3));
    let SegmentOutcome::Decode(first) = poll(&mut segmenter, &buffer).expect("the segment closes")
    else {
        panic!("ordinary speech is decoded");
    };
    assert_eq!(first.start, 0);
    // Polling again without new audio returns nothing.
    assert!(poll(&mut segmenter, &buffer).is_none());
}

#[test]
fn compacted_buffers_keep_absolute_segment_ranges() {
    let mut buffer = take(&[speech(1), silence(3)]);
    let mut segmenter = energy();
    let SegmentOutcome::Decode(first) =
        poll(&mut segmenter, &buffer).expect("the first absolute segment closes")
    else {
        panic!("ordinary speech decodes");
    };
    let first_end = usize::try_from(first.end).expect("test range fits");
    buffer.drain(..first_end);
    buffer.extend(take(&[speech(1), silence(3)]));

    segmenter
        .classify(&buffer, first.end, true)
        .expect("classified");
    let SegmentOutcome::Decode(second) = segmenter.poll().expect("the compacted segment closes")
    else {
        panic!("ordinary speech decodes");
    };
    assert!(second.start >= first.end);
    assert_eq!(segmenter.consumed(), second.end);
}

#[test]
fn continuous_speech_forces_exact_absolute_strides_with_bounded_overlap() {
    let buffer = speech(21);
    let mut segmenter = energy();

    let SegmentOutcome::Forced(first) =
        poll(&mut segmenter, &buffer).expect("313 frames force the first final window")
    else {
        panic!("continuous speech uses an explicit forced boundary");
    };
    assert_eq!(first.decode_range(), 0..160_256);
    assert_eq!(first.new_audio(), 0..160_256);
    assert_eq!(first.overlap(), None);

    let SegmentOutcome::Forced(second) = segmenter
        .poll()
        .expect("the next 313 frames force another final window")
    else {
        panic!("later continuous speech retains forced metadata");
    };
    assert_eq!(second.decode_range(), 32_256..320_512);
    assert_eq!(second.new_audio(), 160_256..320_512);
    assert_eq!(second.overlap(), Some(32_256..160_256));
    assert_eq!(segmenter.consumed(), 320_512);
}

#[test]
fn first_natural_boundary_keeps_then_resets_forced_overlap_ownership() {
    let buffer = take(&[speech(10), speech(1), silence(3), speech(10)]);
    let mut segmenter = energy();
    assert!(matches!(
        poll(&mut segmenter, &buffer),
        Some(SegmentOutcome::Forced(_))
    ));
    let SegmentOutcome::Forced(natural) = segmenter
        .poll()
        .expect("the first natural boundary retains forced overlap")
    else {
        panic!("the forced successor holds reconciliation metadata");
    };
    assert_eq!(natural.overlap(), Some(32_256..160_256));
    assert_eq!(natural.new_audio().start, 160_256);
    assert!(natural.new_audio().end < 320_000);

    let SegmentOutcome::Forced(after_silence) = segmenter
        .poll()
        .expect("the next continuous run reaches its own forced boundary")
    else {
        panic!("speech after a natural boundary is forced independently");
    };
    assert_eq!(after_silence.overlap(), None);
    assert_eq!(after_silence.decode_range(), after_silence.new_audio());
}

#[test]
fn natural_segment_ends_a_hangover_past_the_silence_and_closes_after_two_seconds() {
    let (mut segmenter, _) = scripted(&[(0, 38_400)]);
    assert_eq!(
        hear(&mut segmenter, 70_655),
        None,
        "silence from 38,400 closes only once a frame reaches two seconds past it"
    );
    assert_eq!(
        hear(&mut segmenter, 70_656),
        Some(SegmentOutcome::Decode(0..40_000)),
        "the segment keeps 100 ms of the closing silence"
    );
    assert_eq!(segmenter.consumed(), 40_000);
}

#[test]
fn a_sentence_end_hint_closes_a_segment_after_two_tenths_of_a_second() {
    let (mut unhinted, _) = scripted(&[(0, 38_400)]);
    assert_eq!(
        hear(&mut unhinted, 41_984),
        None,
        "without the hint 0.2 s of silence is a pause"
    );
    let (mut segmenter, _) = scripted(&[(0, 38_400)]);
    segmenter.set_sentence_end(0, true);
    assert_eq!(hear(&mut segmenter, 41_983), None);
    assert_eq!(
        hear(&mut segmenter, 41_984),
        Some(SegmentOutcome::Decode(0..40_000)),
        "silence from 38,400 closes once a frame reaches 0.2 s past it"
    );
}

#[test]
fn a_sentence_end_hint_lapses_once_its_segment_closes() {
    let (mut segmenter, _) = scripted(&[(0, 38_400), (48_128, 86_528)]);
    segmenter.set_sentence_end(0, true);
    assert_eq!(
        hear(&mut segmenter, 48_128),
        Some(SegmentOutcome::Decode(0..40_000))
    );
    assert_eq!(
        hear(&mut segmenter, 118_783),
        None,
        "the next segment has no accepted interim text, so its pause is not a sentence end"
    );
    assert_eq!(
        hear(&mut segmenter, 118_784),
        Some(SegmentOutcome::Decode(40_128..88_128))
    );
}

#[test]
fn natural_segment_starts_half_a_second_before_its_speech() {
    let (mut segmenter, _) = scripted(&[(24_576, 62_976)]);
    assert_eq!(
        hear(&mut segmenter, 110_976),
        Some(SegmentOutcome::Decode(16_576..64_576))
    );
}

#[test]
fn pre_roll_never_reaches_before_the_start_of_the_take() {
    let (mut segmenter, _) = scripted(&[(3_072, 41_472)]);
    assert_eq!(
        hear(&mut segmenter, 89_472),
        Some(SegmentOutcome::Decode(0..43_072))
    );
}

#[test]
fn pre_roll_after_a_paused_stride_stops_at_the_stride_and_starts_a_fresh_segment() {
    let (mut segmenter, _) = scripted(&[(0, 158_208), (160_768, 199_168)]);
    let Some(SegmentOutcome::Forced(stride)) = hear(&mut segmenter, 247_168) else {
        panic!("313 frames after the onset force a stride");
    };
    assert_eq!(stride.decode_range(), 0..160_256);
    assert_eq!(
        segmenter.poll(),
        Some(SegmentOutcome::Decode(160_256..200_768)),
        "speech resumed after a pause does not overlap the stride it follows"
    );
}

#[test]
fn a_forced_stride_closes_on_the_frame_grid_and_overlaps_its_successor_by_eight_seconds() {
    let (mut segmenter, _) = scripted(&[(3_072, 400_000)]);
    assert_eq!(hear(&mut segmenter, 163_327), None);
    let Some(SegmentOutcome::Forced(first)) = hear(&mut segmenter, 163_328) else {
        panic!("10.016 s after the onset force a stride");
    };
    assert_eq!(first.decode_range(), 0..163_328);
    assert_eq!(first.new_audio(), 0..163_328);
    let Some(SegmentOutcome::Forced(second)) = hear(&mut segmenter, 323_584) else {
        panic!("continuous speech forces the next stride");
    };
    assert_eq!(second.overlap(), Some(35_328..163_328));
    assert_eq!(second.new_audio(), 163_328..323_584);
    assert_eq!(
        second.new_audio().end % FRAME,
        0,
        "a stride ends on a frame boundary, so the grid never restarts"
    );
}

#[test]
fn a_word_shorter_than_the_final_window_decodes_with_its_pre_roll_and_hangover() {
    let (mut segmenter, _) = scripted(&[(24_576, 31_232)]);
    assert_eq!(
        hear(&mut segmenter, 79_232),
        Some(SegmentOutcome::Decode(16_576..32_832)),
        "a 416 ms run is past the click rule, so the final pass decodes it"
    );
    assert_eq!(segmenter.consumed(), 32_832);
}

#[test]
fn a_short_word_without_its_whole_pre_roll_extends_into_its_closing_silence() {
    let (mut segmenter, _) = scripted(&[(1_024, 5_632)]);
    assert_eq!(
        hear(&mut segmenter, 53_632),
        Some(SegmentOutcome::Decode(0..8_000)),
        "a 288 ms run at the start of the take reaches the final window in scanned silence"
    );
    assert_eq!(segmenter.consumed(), 8_000);
}

#[test]
fn the_click_rule_skips_runs_under_250_ms_measured_without_padding() {
    let (mut click, _) = scripted(&[(24_576, 28_160)]);
    assert_eq!(
        hear(&mut click, 76_160),
        Some(SegmentOutcome::Skipped(16_576..29_760)),
        "a 224 ms run is a click even though its padded segment is longer"
    );
    assert_eq!(click.consumed(), 29_760);
    let (mut word, _) = scripted(&[(24_576, 28_672)]);
    assert_eq!(
        hear(&mut word, 76_672),
        Some(SegmentOutcome::Decode(16_576..30_272)),
        "a 256 ms run holds a word"
    );
}

#[test]
fn a_click_closes_at_the_sentence_end_silence_and_is_still_skipped() {
    let (mut segmenter, _) = scripted(&[(24_576, 28_160)]);
    assert_eq!(hear(&mut segmenter, 31_743), None);
    assert_eq!(
        hear(&mut segmenter, 31_744),
        Some(SegmentOutcome::Skipped(16_576..29_760)),
        "a 224 ms run closes once a frame reaches 0.2 s of silence and is still a click"
    );
}

#[test]
fn a_silence_close_proves_only_the_frames_it_decided_on_silent() {
    let (mut segmenter, _) = scripted(&[(24_576, 28_160), (61_440, 99_840)]);
    assert_eq!(
        hear(&mut segmenter, 147_840),
        Some(SegmentOutcome::Skipped(16_576..29_760))
    );
    assert_eq!(
        segmenter.scanned(),
        31_744,
        "the frame that reached the closing silence ends the proven-silent span, \
         though later speech is already classified"
    );
}

#[test]
fn every_chunk_is_classified_once_in_order_across_two_strides() {
    let (mut segmenter, detector) = scripted(&[(0, 340_000)]);
    let mut buffer = Vec::new();
    let mut strides = Vec::new();
    while buffer.len() < 336_000 {
        buffer.resize(buffer.len() + 1_000, 0.0);
        segmenter.classify(&buffer, 0, true).expect("classified");
        while let Some(outcome) = segmenter.poll() {
            if let SegmentOutcome::Forced(stride) = outcome {
                strides.push(stride.new_audio());
            }
        }
    }
    assert_eq!(strides, [0..160_256, 160_256..320_512]);
    let starts = (0..336_000 / FRAME_SAMPLES)
        .map(|chunk| chunk * FRAME_SAMPLES)
        .collect::<Vec<_>>();
    assert_eq!(detector.chunk_starts(), starts);
}

#[test]
fn speech_before_reads_classified_speech_without_a_poll() {
    let (mut segmenter, _) = scripted(&[(0, 24_000)]);
    segmenter
        .classify(&vec![0.0; 32_000], 0, false)
        .expect("classified");
    let speech = segmenter
        .speech_before(32_000)
        .expect("every whole frame before the end is classified");
    assert_eq!(
        speech.after(0),
        24_064 - HANGOVER_SAMPLES,
        "speech ends with the frame the run reaches into"
    );
    assert!(
        segmenter.speech_before(32_513).is_none(),
        "a whole frame before the end is unclassified"
    );
}

#[test]
fn the_speech_record_holds_every_classified_speech_frame_merged_into_runs() {
    let (mut segmenter, _) = scripted(&[(600, 1_100), (2_100, 2_200)]);
    let mut buffer = vec![0.0; 1_100];
    segmenter.classify(&buffer, 0, false).expect("classified");
    assert_eq!(
        segmenter.speech_runs(),
        std::slice::from_ref(&(FRAME..2 * FRAME)),
        "the first call classifies frames 0 and 1, so the run straddles the calls"
    );
    buffer.resize(5 * FRAME_SAMPLES + 100, 0.0);
    segmenter.classify(&buffer, 0, false).expect("classified");
    assert_eq!(
        segmenter.speech_runs(),
        [FRAME..3 * FRAME, 4 * FRAME..5 * FRAME],
        "speech inside a frame records that whole frame, adjacent frames merge across calls, \
         and the partial last frame is not yet classified"
    );
}

#[test]
fn a_take_without_a_final_pipeline_queues_no_decisions() {
    let audio = vec![0.0; 84_000];
    let (mut queued, _) = scripted(&[(0, 24_000)]);
    queued.classify(&audio, 0, true).expect("classified");
    assert_eq!(
        queued.poll(),
        Some(SegmentOutcome::Decode(0..25_664)),
        "queued, the run closes two seconds into its silence"
    );
    let (mut unqueued, _) = scripted(&[(0, 24_000)]);
    unqueued.classify(&audio, 0, false).expect("classified");
    assert_eq!(
        unqueued.poll(),
        None,
        "unqueued, the same decisions close nothing"
    );
}
