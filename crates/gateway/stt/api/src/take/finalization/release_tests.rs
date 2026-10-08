//! Tests for closed ranges the retained PCM cap releases while the final
//! queue is full, whose accepted interim text becomes final.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use gateway_stt_engine::DecodeRequest;
use tokio::sync::mpsc;

use super::tests::{closed_speech, queue, saturate};
use super::{ClosedRange, FINAL_SEGMENT_CAPACITY, FinalSegmentOwner, run_final_pipeline};
use crate::audio::AudioError;
use crate::take::state::TakeState;
use crate::take::{SPEECH_TAIL_SAMPLES, Take};

/// A take whose final queue stays full, so every closed range is held, and
/// whose pipeline reports each decode's sample count once completion drains
/// the held ranges.
fn held_take(limit: usize) -> (Take, Vec<FinalSegmentOwner>, mpsc::UnboundedReceiver<usize>) {
    let (commands, receiver) = mpsc::channel(FINAL_SEGMENT_CAPACITY);
    let pending = Arc::new(AtomicUsize::new(0));
    let mut take = Take::with_pcm_limit(Vec::new(), None, limit);
    let (report, decodes) = mpsc::unbounded_channel();
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        Arc::from([]),
        Arc::clone(&take.state),
        Arc::clone(&take.whole_window),
        move |request: DecodeRequest| {
            let _ = report.send(request.samples().len());
            async { Some(Ok("decoded".to_owned())) }
        },
    ));
    take.final_pipeline = Some(queue(commands, task, Arc::clone(&pending)));
    (take, saturate(&pending), decodes)
}

/// Appends continuous speech in 200 ms chunks through `end`, polling the
/// segmenter after each so forced strides close and are held.
fn speak_through(take: &Take, end: u64) {
    let mut appended = TakeState::lock(&take.state.buffer).end();
    while appended < end {
        take.append(vec![0.5; 3_200])
            .expect("the cap releases the oldest held range");
        take.submit_closed_segments();
        appended += 3_200;
    }
    assert!(take.pending_failure().is_none());
}

async fn finish(take: &Take) -> String {
    take.finalization()
        .expect("the take owns a final pipeline")
        .await
        .expect("completion succeeds")
}

#[tokio::test]
async fn the_pcm_cap_releases_a_held_range_and_keeps_its_interim_text_final() {
    let (take, _held, mut decodes) = held_take(120_000);
    take.append(closed_speech()).expect("speech PCM reserves");
    take.next_window_snapshot("keep these words", &[], 0, 0, 16_000)
        .expect("the interim hypothesis is accepted");
    take.submit_closed_segments();

    let mut appended = 64_000;
    while take.append(vec![0.0; 1_600]).is_ok() {
        appended += 1_600;
    }
    assert!(
        appended > 120_000,
        "small appends release the held range at the cap, appended {appended}"
    );
    assert!(take.pending_failure().is_none());
    assert!(
        matches!(
            take.append(vec![0.0; 1_600]),
            Err(AudioError::BufferTooLong { .. })
        ),
        "with no held range left the cap rejects the append"
    );

    assert_eq!(finish(&take).await, "keep these words");
    assert!(decodes.try_recv().is_err(), "nothing decodes");
}

#[tokio::test]
async fn a_released_silence_close_keeps_the_text_of_a_window_ending_at_its_end() {
    let (take, _held, mut decodes) = held_take(120_000);
    take.append(closed_speech()).expect("speech PCM reserves");
    let window = take.interim_window(usize::MAX).expect("the window copies");
    take.next_window_snapshot(
        "keep these words",
        &[],
        window.segment_start,
        window.start,
        window.end,
    )
    .expect("the interim hypothesis is accepted");
    take.submit_closed_segments();
    let closed_end = match TakeState::lock(&take.state.held).front() {
        Some(ClosedRange::Segment {
            range,
            forced: None,
            ..
        }) => range.end,
        other => panic!("the silence close is held, found {other:?}"),
    };
    assert_eq!(
        closed_end, window.end,
        "the interim window ends with the silence close"
    );

    while take.append(vec![0.0; 1_600]).is_ok() {}
    assert!(
        matches!(
            TakeState::lock(&take.state.held).front(),
            Some(ClosedRange::Released { .. })
        ),
        "the cap released the silence close"
    );
    assert!(take.pending_failure().is_none());

    assert_eq!(finish(&take).await, "keep these words");
    assert!(decodes.try_recv().is_err(), "nothing decodes");
}

#[tokio::test]
async fn a_released_silence_close_keeps_the_text_of_a_window_running_into_its_silence() {
    let (take, _held, mut decodes) = held_take(120_000);
    take.append(closed_speech()).expect("speech PCM reserves");
    let window = take.interim_window(usize::MAX).expect("the window copies");
    let window_end = window.end + SPEECH_TAIL_SAMPLES;
    take.next_window_snapshot(
        "keep these words",
        &[],
        window.segment_start,
        window.start,
        window_end,
    )
    .expect("the interim hypothesis is accepted");
    take.submit_closed_segments();
    let (closed_end, silent_through) = match TakeState::lock(&take.state.held).front() {
        Some(ClosedRange::Segment {
            range,
            forced: None,
            silent_through,
            ..
        }) => (range.end, *silent_through),
        other => panic!("the silence close is held, found {other:?}"),
    };
    assert!(
        closed_end < window_end && window_end <= silent_through,
        "the interim window runs past {closed_end} into the silence heard through {silent_through}"
    );

    while !matches!(
        TakeState::lock(&take.state.held).front(),
        Some(ClosedRange::Released { .. })
    ) {
        take.append(vec![0.5; 3_200])
            .expect("the cap releases the silence close");
    }
    assert!(take.pending_failure().is_none());

    assert_eq!(
        finish(&take).await,
        "keep these words decoded",
        "the decoded speech after the released range cannot take over the window's text"
    );
    assert!(
        decodes.recv().await.is_some(),
        "the speech after the release decodes"
    );
}

#[tokio::test]
async fn the_pcm_cap_releases_a_held_stride_and_its_held_successor_decodes_alone() {
    let (take, _held, mut decodes) = held_take(400_000);
    speak_through(&take, 320_512);
    assert_eq!(TakeState::lock(&take.state.held).len(), 2);
    speak_through(&take, 400_000);
    assert_eq!(TakeState::lock(&take.state.buffer).origin(), 160_256);
    take.next_window_snapshot("first stride words", &[], 0, 0, 160_256)
        .expect("the first stride's hypothesis is accepted");

    let transcript = finish(&take).await;
    assert!(
        transcript.starts_with("first stride words "),
        "{transcript}"
    );
    assert!(transcript.ends_with("decoded"), "{transcript}");
    assert_eq!(
        decodes.recv().await,
        Some(160_256),
        "the successor decodes its new audio without the released overlap"
    );
    assert_eq!(decodes.recv().await, Some(207_488), "the tail overlaps it");
}

#[tokio::test]
async fn a_released_stride_keeps_the_text_of_a_window_running_past_its_end() {
    let (take, _held, mut decodes) = held_take(400_000);
    speak_through(&take, 400_000);
    assert_eq!(TakeState::lock(&take.state.buffer).origin(), 160_256);
    take.next_window_snapshot("first stride words", &[], 0, 0, 163_200)
        .expect("a window running past the stride end is accepted");

    let transcript = finish(&take).await;
    assert!(
        transcript.starts_with("first stride words "),
        "{transcript}"
    );
    assert!(transcript.ends_with("decoded"), "{transcript}");
    assert_eq!(
        decodes.recv().await,
        Some(160_256),
        "the successor still decodes the audio after the stride"
    );
    assert_eq!(decodes.recv().await, Some(207_488), "the tail overlaps it");
}

#[tokio::test]
async fn a_stride_after_a_released_stride_starts_without_overlap() {
    let (take, _held, mut decodes) = held_take(240_000);
    speak_through(&take, 416_000);
    assert_eq!(TakeState::lock(&take.state.buffer).origin(), 320_512);
    take.next_window_snapshot("first stride words", &[], 0, 0, 160_256)
        .expect("the first stride's hypothesis is accepted");
    take.next_window_snapshot("second stride words", &[], 160_256, 160_256, 320_512)
        .expect("the second stride's hypothesis is accepted");

    assert_eq!(
        finish(&take).await,
        "first stride words second stride words decoded"
    );
    assert_eq!(
        decodes.recv().await,
        Some(95_488),
        "the tail after the released strides decodes as a natural segment"
    );
    assert_eq!(decodes.recv().await, None);
}
