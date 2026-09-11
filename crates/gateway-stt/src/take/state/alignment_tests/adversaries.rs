use std::sync::Arc;

use super::{FinalRangeOutcome, ForcedBoundary, TakeState};
use crate::take::TakeFailure;

fn completion(
    first_range: std::ops::Range<u64>,
    overlap: std::ops::Range<u64>,
    new_audio: std::ops::Range<u64>,
    previous: &str,
    current: &str,
) -> Result<String, Arc<TakeFailure>> {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(ForcedBoundary::first(first_range), previous.to_owned()),
        &[],
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(overlap, new_audio.clone()),
            current.to_owned(),
        ),
        &[],
    );
    state.completion(&[], new_audio.end)
}

fn assert_estimated(result: Result<String, Arc<TakeFailure>>, expected: &str) {
    assert_eq!(
        result.expect("weak overlap uses bounded projection"),
        expected
    );
}

#[test]
fn common_of_the_at_opposite_projected_edges_uses_range_ownership() {
    assert_estimated(
        completion(
            0..160_000,
            32_000..160_000,
            160_000..320_000,
            "private alpha beta gamma delta epsilon zeta eta theta kappa of the",
            "of the wholly unrelated current text",
        ),
        "private alpha of the wholly unrelated current text",
    );
}

#[test]
fn nonuniform_speech_density_uses_the_exact_audio_fraction() {
    assert_estimated(
        completion(
            0..160_000,
            32_000..160_000,
            160_000..320_000,
            "a b c d e f g h i j k l red orange yellow green blue indigo violet white",
            "red orange yellow green blue indigo violet white unrelated tail words continue now",
        ),
        "a b c d red orange yellow green blue indigo violet white unrelated tail words continue now",
    );
}

#[test]
fn similarly_scored_repeated_choruses_use_deterministic_projection() {
    assert_estimated(
        completion(
            0..90,
            30..90,
            90..120,
            "intro chorus one two three chorus one two three",
            "chorus one two three chorus one two three outro",
        ),
        "intro chorus one chorus one two three chorus one two three outro",
    );
}

#[test]
fn punctuation_only_revisions_continue_without_whole_window_duplication() {
    assert_estimated(
        completion(
            0..160_000,
            32_000..160_000,
            160_000..320_000,
            "... ?! ;;; ,,, !!!",
            "??? !!!",
        ),
        "... ??? !!!",
    );
}

#[test]
fn an_over_limit_final_transcript_fails_before_it_can_become_pending() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(ForcedBoundary::first(0..160_000), "x".repeat(16 * 1024 + 1)),
        &[],
    );

    let failure = state
        .completion(&[], 160_000)
        .expect_err("over-limit transcript fails");
    assert!(matches!(&*failure, TakeFailure::TranscriptLimit));
    assert_eq!(
        failure.to_string(),
        "final transcript exceeds the 16 KiB window limit"
    );
}
