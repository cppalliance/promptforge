//! Tests for the aligned rewrite of natural finals over displayed interim text.

use std::sync::Mutex;

use super::super::TakeState;
use crate::take::InterimSnapshot;
use crate::take::final_outcome::{FinalRangeOutcome, SkipReason};
use crate::take::finalization::record_outcome;
use crate::take::live_prefix::AnchoredSuffix;
use crate::take::window::{AcceptedHypothesis, ShownHypotheses, WholeWindowState};

const FINAL_END: u64 = 32_000;
const HALF_SECOND: u64 = 8_000;
const FINALIZED: &str = "Ask not what your country";

type Parts = (String, String, String, String);

struct Rewrite {
    state: TakeState,
    window: Mutex<WholeWindowState>,
}

/// Renders the `interim` passes over `0..FINAL_END + n * HALF_SECOND` for
/// their positions n from 1, so the window always ends past the final, then
/// records `final_text` as the natural final through `FINAL_END` with the
/// production caller.
fn rewrite(interim: &[&str], final_text: &str) -> Rewrite {
    let rewrite = Rewrite {
        state: TakeState::default(),
        window: Mutex::new(WholeWindowState::default()),
    };
    for (step, hypothesis) in (1..).zip(interim) {
        rewrite.render(0, FINAL_END + step * HALF_SECOND, hypothesis);
    }
    record_outcome(
        &rewrite.state,
        &rewrite.window,
        FinalRangeOutcome::decoded(0..FINAL_END, final_text.to_owned()),
    );
    rewrite
}

impl Rewrite {
    fn render(&self, start: u64, end: u64, hypothesis: &str) -> Option<Parts> {
        TakeState::lock(&self.window)
            .try_next(
                &self.state.live_prefix_snapshot(),
                start,
                start,
                end,
                hypothesis,
            )
            .expect("the window stays within capacity")
            .map(InterimSnapshot::into_parts)
    }

    /// Recomposes the shown snapshot after the final without a fast pass.
    fn refresh(&self) -> Parts {
        TakeState::lock(&self.window)
            .refresh(&self.state.live_prefix_snapshot())
            .into_parts()
    }

    /// Renders the fast pass `step` half seconds after the final's end.
    fn next(&self, step: u64, hypothesis: &str) -> Parts {
        let parts = self
            .render(FINAL_END, FINAL_END + step * HALF_SECOND, hypothesis)
            .unwrap_or_else(|| panic!("{hypothesis:?} emits"));
        assert_eq!(parts.0, format!("{}{}{}", parts.1, parts.2, parts.3));
        parts
    }
}

fn parts(agreed: &str, tentative: &str) -> Parts {
    (
        format!("{FINALIZED}{agreed}{tentative}"),
        FINALIZED.to_owned(),
        agreed.to_owned(),
        tentative.to_owned(),
    )
}

#[test]
fn an_interim_window_ending_past_the_final_anchors_its_displayed_suffix() {
    let rewrite = rewrite(&["ask not what your country can do"], FINALIZED);

    assert_eq!(
        rewrite.state.finalized_snapshot(),
        (FINALIZED.to_owned(), FINAL_END),
        "the final stays authoritative for its range"
    );
    assert_eq!(
        rewrite
            .state
            .live_prefix_snapshot()
            .anchored()
            .map(AnchoredSuffix::text),
        Some("can do")
    );
    assert_eq!(rewrite.next(1, "for you"), parts("", " can do for you"));
    let accepted = TakeState::lock(&rewrite.window).accepted_hypotheses(FINAL_END);
    assert_eq!(
        rewrite
            .state
            .completion(&accepted, FINAL_END)
            .expect("the rewritten take completes"),
        FINALIZED,
        "kept interim words never enter the completed transcript"
    );
}

#[test]
fn a_final_differing_in_one_word_changes_one_displayed_word() {
    let interim = "ask not what your country can do for you";
    let rewrite = rewrite(&[interim], "ask not what your county can do");

    let transcript = rewrite.next(1, "ask what").0;
    let changed = transcript
        .split_whitespace()
        .zip(interim.split_whitespace())
        .filter(|(after, before)| after != before)
        .count();
    assert_eq!(
        transcript,
        "ask not what your county can do for you ask what"
    );
    assert_eq!(changed, 1, "only the corrected word changes");
}

#[test]
fn anchored_suffix_is_capped_at_five_words() {
    let rewrite = rewrite(
        &["we choose to go to the moon in this decade and do the other"],
        "we choose to go to the moon",
    );

    let (_, finalized, _, tentative) = rewrite.next(1, "things");
    assert_eq!(finalized, "we choose to go to the moon");
    assert_eq!(tentative, " in this decade and do things");
}

#[test]
fn unanchored_displayed_words_after_the_final_are_dropped() {
    let rewrite = rewrite(&["ask not what your nation can do"], FINALIZED);

    assert_eq!(rewrite.next(1, "for you"), parts("", " for you"));
}

#[test]
fn a_fast_pass_that_rederives_the_suffix_shows_it_once() {
    let rewrite = rewrite(&["ask not what your country can do"], FINALIZED);

    assert_eq!(
        rewrite.next(1, "can do for you"),
        parts("", " can do for you")
    );
}

#[test]
fn a_fast_pass_that_revises_one_suffix_word_replaces_the_suffix_once() {
    let rewrite = rewrite(&["ask not what your country can do"], FINALIZED);

    assert_eq!(
        rewrite.next(1, "could do for you"),
        parts("", " could do for you")
    );
}

#[test]
fn suffix_words_agreed_before_the_final_stay_agreed() {
    let rewrite = rewrite(
        &[
            "ask not what your country can",
            "ask not what your country can do",
        ],
        FINALIZED,
    );

    assert_eq!(
        rewrite.next(1, "can do for you"),
        parts(" can", " do for you")
    );
}

#[test]
fn a_pass_disputing_an_agreed_suffix_word_does_not_shrink_agreed_text() {
    let interim = "ask not what your country can do";
    let rewrite = rewrite(&[interim, interim], FINALIZED);

    assert_eq!(
        rewrite.next(1, "can do for you"),
        parts(" can do", " for you")
    );
    assert_eq!(
        rewrite.next(2, "could do for you"),
        parts(" can do for you", "")
    );
}

#[test]
fn a_window_agreeing_on_rederived_suffix_words_shows_them_once() {
    let rewrite = rewrite(&["ask not what your country can do"], FINALIZED);

    rewrite.next(1, "for you");
    assert_eq!(
        rewrite.next(2, "can do for you"),
        parts(" can do for you", "")
    );
}

#[test]
fn an_anchored_suffix_seeds_the_next_window_once() {
    let rewrite = rewrite(&["ask not what your country can do"], FINALIZED);

    rewrite.next(1, "for you");
    assert_eq!(rewrite.next(2, ""), parts("", ""));
    assert_eq!(
        rewrite.next(3, "for you"),
        parts("", " for you"),
        "the suffix stays gone once a fast pass drops it"
    );
}

#[test]
fn a_refresh_after_the_final_shows_the_anchored_suffix_and_leaves_it_to_seed_the_next_pass() {
    let rewrite = rewrite(&["ask not what your country can do"], FINALIZED);

    assert_eq!(rewrite.refresh(), parts("", " can do"));
    assert_eq!(rewrite.next(1, "for you"), parts("", " can do for you"));
}

#[test]
fn a_refresh_keeps_anchored_words_agreed_before_the_final_agreed() {
    let rewrite = rewrite(
        &[
            "ask not what your country can",
            "ask not what your country can do",
        ],
        FINALIZED,
    );

    assert_eq!(rewrite.refresh(), parts(" can", " do"));
}

#[test]
fn a_refresh_after_an_unanchored_final_shows_only_the_finalized_text() {
    let rewrite = rewrite(&["ask not what your nation can do"], FINALIZED);

    assert_eq!(rewrite.refresh(), parts("", ""));
}

#[test]
fn a_refresh_after_a_skip_without_text_keeps_the_word_shown_and_accepted() {
    let rewrite = rewrite(&["ask not what your country"], FINALIZED);
    assert_eq!(rewrite.next(3, "Hey."), parts("", " Hey."));
    record_outcome(
        &rewrite.state,
        &rewrite.window,
        FinalRangeOutcome::skipped(
            FINAL_END..FINAL_END + 2 * HALF_SECOND,
            SkipReason::BelowSpeechThreshold,
        ),
    );

    assert_eq!(
        rewrite.refresh(),
        parts("", " Hey."),
        "no settled text holds the word yet"
    );
    let accepted =
        TakeState::lock(&rewrite.window).accepted_hypotheses(FINAL_END + 3 * HALF_SECOND);
    assert_eq!(
        accepted.last().map(AcceptedHypothesis::text),
        Some("Hey."),
        "a later skipped range can still settle the word"
    );
}

#[test]
fn a_following_outcome_retires_the_anchored_suffix() {
    let rewrite = rewrite(&["ask not what your country can do"], FINALIZED);
    assert!(rewrite.state.live_prefix_snapshot().anchored().is_some());

    rewrite.state.record_final_outcome(
        FinalRangeOutcome::skipped(FINAL_END..FINAL_END + HALF_SECOND, SkipReason::Silence),
        &[],
        &ShownHypotheses::default(),
    );

    assert_eq!(rewrite.state.live_prefix_snapshot().anchored(), None);
    assert_eq!(
        rewrite
            .render(
                FINAL_END + HALF_SECOND,
                FINAL_END + 2 * HALF_SECOND,
                "for you"
            )
            .map(|parts| parts.0),
        Some(format!("{FINALIZED} for you"))
    );
}
