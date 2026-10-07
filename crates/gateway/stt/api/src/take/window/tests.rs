//! Tests for whole-window interim revision and accepted-hypothesis retention.

use super::{MAX_PENDING_ACCEPTED_HYPOTHESES, WholeWindowState};
use crate::take::final_outcome::{FinalRangeOutcome, SkipReason, assemble_completion};

#[test]
fn whole_window_revision_replaces_an_unpromoted_leading_phrase() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 8_000, "Why is it");
    state.next("", 0, 0, 0, 9_600, "Why is it");
    let snapshot = state
        .next("", 0, 0, 0, 11_200, "Why is this")
        .expect("a revised whole-window hypothesis emits");

    assert_eq!(
        snapshot.into_parts(),
        (
            "Why is this".to_owned(),
            String::new(),
            String::new(),
            "Why is this".to_owned(),
        )
    );
}

#[test]
fn agreed_text_does_not_shrink_when_a_later_hypothesis_revises_an_agreed_word() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 8_000, "Why is it");
    let agreed = state
        .next("", 0, 0, 0, 16_000, "Why is it")
        .expect("a repeated hypothesis agrees on every word")
        .into_parts()
        .2;
    assert_eq!(agreed, "Why is it");

    let revised = state
        .next("", 0, 0, 0, 24_000, "Why is this")
        .expect("a revised hypothesis emits")
        .into_parts();
    assert_eq!(revised.0, "Why is it");
    assert_eq!(revised.2, "Why is it");
}

#[test]
fn advancing_window_replaces_a_revision_with_two_equivalent_leading_tokens() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 16_000, "Why, IS it");
    let snapshot = state
        .next("", 0, 0, 8_000, 24_000, "why is this")
        .expect("a trustworthy leading prefix permits replacement");

    assert_eq!(snapshot.into_parts().0, "why is this");
    let accepted = state.accepted_hypotheses(24_000);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].range(), 0..24_000);
    assert_eq!(accepted[0].text(), "why is this");
}

#[test]
fn advancing_window_rejects_one_generic_equivalent_leading_token() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 16_000, "And this stays");

    assert!(
        state
            .next("", 0, 0, 8_000, 24_000, "and unrelated fragment")
            .is_none()
    );
    let accepted = state.accepted_hypotheses(16_000);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].range(), 0..16_000);
    assert_eq!(accepted[0].text(), "And this stays");
}

#[test]
fn consumed_boundary_starts_a_region_before_finalization_arrives() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 16_000, "first segment");
    state.next("", 0, 0, 0, 16_000, "first segment");
    let pending = state
        .next("", 0, 16_000, 16_000, 24_000, "second start")
        .expect("the new segment starts without waiting for final text");
    assert_eq!(pending.into_parts().0, "first segment second start");

    let authoritative = state
        .next(
            "revised first",
            16_000,
            16_000,
            16_000,
            25_600,
            "second start now",
        )
        .expect("authoritative text replaces the pending segment");
    assert_eq!(
        authoritative.into_parts().0,
        "revised first second start now"
    );
}

#[test]
fn advancing_window_rebases_through_overlap_without_repeating_it() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 16_000, "ask not what your country can do");
    let snapshot = state
        .next("", 0, 0, 8_000, 24_000, "your country can do for you")
        .expect("the sliding window emits a rebased hypothesis");

    assert_eq!(
        snapshot.into_parts().0,
        "ask not what your country can do for you"
    );
}

#[test]
fn no_overlap_waits_for_disjoint_audio_before_appending_once() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 128_000, "alpha beta");
    assert!(state.next("", 0, 0, 8_000, 136_000, "gamma").is_none());
    assert!(state.next("", 0, 0, 16_000, 144_000, "delta").is_none());
    let snapshot = state
        .next("", 0, 0, 160_000, 256_000, "gamma delta")
        .expect("disjoint audio appends one new region");
    assert_eq!(snapshot.into_parts().0, "alpha beta gamma delta");

    let revision = state
        .next("", 0, 0, 176_000, 264_000, "delta epsilon")
        .expect("the new region keeps ordinary overlap replacement");
    assert_eq!(revision.into_parts().0, "alpha beta gamma delta epsilon");

    let accepted = state.accepted_hypotheses(264_000);
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].range(), 0..128_000);
    assert_eq!(accepted[0].text(), "alpha beta");
    assert_eq!(accepted[1].range(), 160_000..264_000);
    assert_eq!(accepted[1].text(), "gamma delta epsilon");
}

#[test]
fn finalized_boundary_does_not_retain_the_superseded_region() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 16_000, "earlier words");
    let snapshot = state
        .next(
            "revised earlier",
            16_000,
            48_000,
            48_000,
            56_000,
            "new words",
        )
        .expect("finalized ownership replaces the earlier region");
    assert_eq!(snapshot.into_parts().0, "revised earlier new words");
}

#[test]
fn finalized_watermark_crossing_active_retires_the_whole_hypothesis() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 8_000, 8_000, 24_000, "crossing words");
    let snapshot = state
        .next("authoritative", 16_000, 32_000, 32_000, 40_000, "new words")
        .expect("the finalized crossing emits its replacement");

    assert_eq!(snapshot.into_parts().0, "authoritative new words");
    let accepted = state.accepted_hypotheses(40_000);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].range(), 32_000..40_000);
    assert_eq!(accepted[0].text(), "new words");
}

#[test]
fn active_starting_at_finalized_watermark_remains_owned() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 16_000, 16_000, 24_000, "boundary words");
    let snapshot = state
        .next("authoritative", 16_000, 32_000, 32_000, 40_000, "new words")
        .expect("text at the exact watermark remains live");

    assert_eq!(
        snapshot.into_parts().0,
        "authoritative boundary words new words"
    );
    let accepted = state.accepted_hypotheses(40_000);
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].range(), 16_000..24_000);
    assert_eq!(accepted[1].range(), 32_000..40_000);
}

#[test]
fn repeated_phrase_in_a_disjoint_region_is_appended_once() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 16_000, "echo now");
    let first = state
        .next("", 0, 0, 32_000, 48_000, "echo now")
        .expect("the repeated phrase has disjoint audio ownership");
    assert_eq!(first.into_parts().0, "echo now echo now");

    let revision = state
        .next("", 0, 0, 32_000, 56_000, "echo now please")
        .expect("the repeated region is replaced instead of appended");
    assert_eq!(revision.into_parts().0, "echo now echo now please");
}

#[test]
fn decoded_then_skipped_completion_uses_only_the_disjoint_candidate() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 128_000, "draft first");
    state.next("", 0, 0, 160_000, 256_000, "accepted tail");
    let accepted = state.accepted_hypotheses(256_000);
    let outcomes = [
        FinalRangeOutcome::decoded(0..160_000, "decoded first".to_owned()),
        FinalRangeOutcome::skipped(160_000..256_000, SkipReason::Silence),
    ];

    assert_eq!(
        assemble_completion(&outcomes, &accepted, 256_000),
        "decoded first accepted tail"
    );
}

#[test]
fn sliding_overlap_tolerates_native_punctuation_revision() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 0, 0, 64_000, "And so my fellow Americans, ask");
    let snapshot = state
        .next("", 0, 0, 16_000, 80_000, "my fellow Americans ask not")
        .expect("the punctuated native overlap rebases");

    assert_eq!(
        snapshot.into_parts().0,
        "And so my fellow Americans ask not"
    );
}

#[test]
fn accepted_hypothesis_retains_exact_committed_audio_coverage() {
    let mut state = WholeWindowState::default();
    state.next("", 0, 32_000, 32_000, 40_000, "old");
    state.next("", 0, 32_000, 32_000, 44_800, "last word");

    assert!(
        state.accepted_hypotheses(40_000).is_empty(),
        "a snapshot extending beyond committed audio is not reusable"
    );
    let accepted = state.accepted_hypotheses(44_800);
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].range(), 32_000..44_800);
    assert_eq!(accepted[0].text(), "last word");
}

#[test]
fn accepted_ranges_remain_absolute_beyond_native_indices() {
    let origin = u64::from(u32::MAX) + 16_000;
    let mut state = WholeWindowState::default();
    state.next(
        "",
        origin,
        origin,
        origin,
        origin + 16_000,
        "absolute words",
    );

    let accepted = state.accepted_hypotheses(origin + 16_000);
    let range: std::ops::Range<u64> = accepted[0].range();
    assert_eq!(range, origin..origin + 16_000);
}

#[test]
fn pending_accepted_hypotheses_have_an_exact_bound() {
    let mut state = WholeWindowState::default();
    let live_prefix = crate::take::live_prefix::LivePrefixSnapshot::for_test("", 0, None);
    for index in 0..=MAX_PENDING_ACCEPTED_HYPOTHESES {
        let start = u64::try_from(index).expect("test index fits") * 2;
        let result = state.try_next(&live_prefix, start, start, start + 1, "word");
        if index < MAX_PENDING_ACCEPTED_HYPOTHESES {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
        }
    }
    assert!(state.pending.len() <= MAX_PENDING_ACCEPTED_HYPOTHESES);
}
