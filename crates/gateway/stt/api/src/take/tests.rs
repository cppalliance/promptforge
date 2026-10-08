//! Tests for per-take isolation, sentence-end hints, echo trims, late
//! decodes, and final pipeline ownership.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use gateway_stt_engine::test_fixtures::ScriptedDetector;
use tokio::sync::{mpsc, oneshot};

use super::final_outcome::{FinalRangeOutcome, SkipReason};
use super::window::ShownHypotheses;
use super::{FinalCommand, FinalSegmentOwner, Take, TakeFailure, run_final_pipeline};

#[test]
fn miri_final_segment_reservation_is_exact() {
    let pending = Arc::new(AtomicUsize::new(0));
    let mut owners = Vec::new();
    for _ in 0..super::FINAL_SEGMENT_CAPACITY {
        owners.push(
            FinalSegmentOwner::reserve(&pending).expect("capacity owns the admitted segment"),
        );
    }
    assert!(FinalSegmentOwner::reserve(&pending).is_none());
    assert_eq!(
        pending.load(Ordering::Acquire),
        super::FINAL_SEGMENT_CAPACITY
    );
    drop(owners);
    assert_eq!(pending.load(Ordering::Acquire), 0);
}

#[test]
fn finalized_history_and_guidance_are_isolated_per_take() {
    let first = Take::without_final(vec!["MCP".to_owned()]);
    let second = Take::without_final(vec!["GGUF".to_owned()]);

    first.record_finalized(Ok("ask not".to_owned()));
    second.record_finalized(Ok("what you".to_owned()));

    assert_eq!(first.guidance(), ["MCP"]);
    assert_eq!(first.finalized(), "ask not");
    assert_eq!(second.guidance(), ["GGUF"]);
    assert_eq!(second.finalized(), "what you");
}

#[test]
fn finalized_segments_aggregate_in_arrival_order() {
    let take = Take::without_final(Vec::new());
    take.record_finalized(Ok("ask not".to_owned()));
    take.record_finalized(Ok("what you can do".to_owned()));
    assert_eq!(take.finalized(), "ask not what you can do");
}

fn sentence_end_hinted(take: &Take) -> bool {
    super::TakeState::lock(&take.state.segmenter).ends_sentence()
}

#[test]
fn the_sentence_end_hint_follows_the_latest_accepted_interim_text() {
    let take = Take::without_final(Vec::new());
    take.next_window_snapshot("Ask not.", &[], 0, 0, 16_000)
        .expect("the first hypothesis is accepted");
    assert!(sentence_end_hinted(&take));
    take.next_window_snapshot("Ask not what", &[], 0, 0, 24_000)
        .expect("a longer hypothesis is accepted");
    assert!(!sentence_end_hinted(&take));
    take.next_window_snapshot("Ask not what you can do?", &[], 0, 0, 32_000)
        .expect("a question is accepted");
    assert!(sentence_end_hinted(&take));
    assert!(
        take.next_window_snapshot("Completely unrelated words", &[], 0, 16_000, 40_000)
            .is_none(),
        "a sliding window that shares no words with the active text is rejected"
    );
    assert!(
        sentence_end_hinted(&take),
        "a rejected hypothesis leaves the hint of the last accepted one"
    );
}

#[test]
fn a_sentence_end_hint_decoded_from_a_closed_segment_is_ignored() {
    let take = Take::without_final(Vec::new());
    super::TakeState::lock(&take.state.segmenter).set_consumed_for_test(16_000);
    take.next_window_snapshot("Ask not.", &[], 0, 0, 16_000)
        .expect("a late decode of the closed segment is accepted");
    assert!(!sentence_end_hinted(&take));
    take.next_window_snapshot("What your country can do.", &[], 16_000, 16_000, 32_000)
        .expect("the open segment's hypothesis is accepted");
    assert!(sentence_end_hinted(&take));
}

#[test]
fn a_take_without_a_final_pipeline_cuts_a_repeat_over_scripted_silence() {
    let detector = ScriptedDetector::new([(0, 24_000)]);
    let take = Take::with_detector(Vec::new(), None, Box::new(detector));
    take.append(vec![0.5; 24_000]).expect("audio appends");
    take.next_window_snapshot("create a plan.", &[], 0, 0, 24_000)
        .expect("the sentence is accepted");
    take.append(vec![0.5; 8_000]).expect("audio appends");

    let (snapshot, _) = take
        .next_window_snapshot("create a plan. Create a", &[], 0, 0, 32_000)
        .expect("the hypothesis less its repeat is accepted");
    assert_eq!(
        snapshot.into_parts().0,
        "create a plan.",
        "the detector heard silence after the sentence, however loud the audio"
    );
    assert!(
        sentence_end_hinted(&take),
        "the sentence still ends, so the segment closes after the short silence"
    );
}

#[test]
fn a_late_decode_of_a_window_a_final_already_covers_shows_nothing() {
    let take = Take::without_final(Vec::new());
    take.next_window_snapshot("ask not what", &[], 0, 0, 16_000)
        .expect("the first hypothesis is accepted");
    take.record_finalized_through("Ask not what.", Some(12_000));

    assert_eq!(
        take.next_window_snapshot("ask not what", &[], 0, 0, 20_000),
        None,
        "a window that starts inside settled text would show its words again"
    );
    assert!(
        take.next_window_snapshot("you can", &[], 12_000, 12_000, 28_000)
            .is_some(),
        "a window from the final's end is decoded as before"
    );
}

#[test]
fn a_late_decode_over_audio_skipped_without_text_is_still_accepted() {
    let take = Take::without_final(Vec::new());
    take.next_window_snapshot("Hey.", &[], 0, 0, 16_000)
        .expect("the first hypothesis is accepted");
    take.state.record_final_outcome(
        FinalRangeOutcome::skipped(0..12_000, SkipReason::BelowSpeechThreshold),
        &[],
        &ShownHypotheses::default(),
    );

    assert!(
        take.next_window_snapshot("Hey.", &[], 0, 0, 20_000)
            .is_some(),
        "no settled text holds the skipped word yet"
    );
}

#[test]
fn a_take_retains_its_first_final_failure() {
    let take = Take::without_final(Vec::new());
    take.record_failure(TakeFailure::Recorded("first".to_owned()));
    take.record_failure(TakeFailure::Recorded("second".to_owned()));
    let failure = take.take_failure().expect("the take owns its failure");
    assert_eq!(failure.to_string(), "first");
}

#[tokio::test]
async fn completed_pipeline_releases_its_retained_dependency() {
    let (commands, receiver) = mpsc::channel(super::FINAL_SEGMENT_CAPACITY);
    let state = Arc::new(super::TakeState::default());
    let retained = Arc::new(());
    let weak: Weak<()> = Arc::downgrade(&retained);
    let pipeline_retained = Arc::clone(&retained);
    let task = tokio::spawn(run_final_pipeline(
        receiver,
        Arc::from([]),
        state,
        Arc::new(Mutex::new(super::WholeWindowState::default())),
        move |_| {
            let retained = Arc::clone(&pipeline_retained);
            async move {
                drop(retained);
                Some(Ok(String::new()))
            }
        },
    ));
    drop(retained);
    let (reply, completion) = oneshot::channel();
    commands
        .send(FinalCommand::Complete {
            committed_samples: 0,
            accepted: Vec::new(),
            reply,
        })
        .await
        .expect("completion queues");

    assert_eq!(
        completion
            .await
            .expect("the completion pipeline replies")
            .expect("completion succeeds"),
        String::new()
    );
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("the completed pipeline terminates before the deadline")
        .expect("the completed pipeline task succeeds");
    assert!(
        weak.upgrade().is_none(),
        "pipeline completion releases its retained engine-like dependency"
    );
}
