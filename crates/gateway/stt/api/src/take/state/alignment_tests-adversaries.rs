//! Adversarial tests for projected-prefix reconciliation of weak forced overlaps.

use std::sync::Arc;

use super::{FinalRangeOutcome, ForcedBoundary, ShownHypotheses, TakeState};
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
        &ShownHypotheses::default(),
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(overlap, new_audio.clone()),
            current.to_owned(),
        ),
        &[],
        &ShownHypotheses::default(),
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
            "of the wholly unrelated current text keeps going on",
        ),
        "private alpha of the wholly unrelated current text keeps going on",
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
            "??? !!! ???",
        ),
        "... ??? !!! ???",
    );
}

/// The words `{prefix}{number:02}` for the numbers in `range`, none carrying
/// punctuation.
fn tail_words(prefix: &str, range: std::ops::RangeInclusive<usize>) -> String {
    range
        .map(|number| format!("{prefix}{number:02}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `count` distinct words, `{prefix}01 {prefix}02 ...`.
fn words(prefix: &str, count: usize) -> String {
    tail_words(prefix, 1..=count)
}

/// `count` distinct words, `t01 t02 ...`.
fn numbered_words(count: usize) -> String {
    words("t", count)
}

#[test]
fn a_clause_boundary_before_the_projected_token_never_drops_predecessor_words() {
    // A 0 to 10.27 s window followed by one from 2.27 s: 36_320 / 164_320 of
    // the 32 tokens projects to token 7. The comma at token 4 sits before
    // that, so cutting there would drop "its place is" (tokens 5 to 7), which
    // the successor window never decoded.
    let predecessor = "The report said clearly, its place is to hold the line while the newer system learns every single step before it takes over completely and without any help now and then done";
    assert_eq!(predecessor.split_whitespace().count(), 32);
    let successor = "to hold alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango uniform victor";
    assert_eq!(successor.split_whitespace().count(), 24);

    assert_estimated(
        completion(
            0..164_320,
            36_320..164_320,
            164_320..324_320,
            predecessor,
            successor,
        ),
        &format!("The report said clearly, its place is {successor}"),
    );
}

#[test]
fn a_sparse_successor_keeps_every_predecessor_word() {
    // The successor decoded 5 words over 2.0 to 14.7 s while the predecessor
    // decoded 36 over 0 to 10 s, so it dropped speech the predecessor heard.
    let predecessor = numbered_words(36);

    assert_estimated(
        completion(
            0..160_000,
            32_000..160_000,
            160_000..235_200,
            &predecessor,
            "gamma delta epsilon zeta eta",
        ),
        &format!("{predecessor} zeta eta"),
    );
}

#[test]
fn a_sparse_successor_is_cut_at_the_projected_token_and_never_snaps_to_a_comma() {
    // The same geometry as the test above: 128_000 of the 203_200 samples in
    // the successor's decode range lie before the overlap end, which rounds
    // 5 tokens to token 3, "epsilon". The comma on token 4 sits in the band
    // after it, but a snap there drops "zeta", a word only the successor's
    // new audio carries.
    let predecessor = numbered_words(36);
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(ForcedBoundary::first(0..160_000), predecessor.clone()),
        &[],
        &ShownHypotheses::default(),
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..235_200),
            "gamma delta epsilon zeta, eta".to_owned(),
        ),
        &[],
        &ShownHypotheses::default(),
    );

    let live = state.live_prefix_snapshot();
    assert_eq!(live.finalized(), predecessor);
    assert_eq!(
        live.pending_forced(),
        Some(("zeta, eta", 160_000..235_200)),
        "the kept tail starts at the projected token, comma included"
    );
    assert_eq!(
        state
            .completion(&[], 235_200)
            .expect("the sparse settlement completes"),
        format!("{predecessor} zeta, eta"),
    );
}

#[test]
fn a_sparse_successor_with_two_punctuated_tokens_in_the_band_keeps_the_projected_cut() {
    // Tokens 4 and 5 both end in punctuation, so no single boundary wins and
    // the cut stays at the projected token 3. This guards only the
    // two-candidate fallback; the no-snap behavior is pinned by the comma test
    // above.
    let predecessor = numbered_words(36);

    assert_estimated(
        completion(
            0..160_000,
            32_000..160_000,
            160_000..235_200,
            &predecessor,
            "gamma delta epsilon zeta, eta,",
        ),
        &format!("{predecessor} zeta, eta,"),
    );
}

#[test]
fn a_one_word_successor_keeps_every_predecessor_word() {
    let predecessor = numbered_words(36);

    assert_estimated(
        completion(
            0..160_000,
            32_000..160_000,
            160_000..235_200,
            &predecessor,
            "The",
        ),
        &predecessor,
    );
}

#[test]
fn a_sparse_successor_leaves_pending_text_over_only_the_audio_after_the_overlap() {
    let predecessor = numbered_words(36);
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(ForcedBoundary::first(0..160_000), predecessor.clone()),
        &[],
        &ShownHypotheses::default(),
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..235_200),
            "gamma delta epsilon zeta eta".to_owned(),
        ),
        &[],
        &ShownHypotheses::default(),
    );

    let live = state.live_prefix_snapshot();
    assert_eq!(live.finalized(), predecessor);
    assert_eq!(live.finalized_samples(), 160_000);
    assert_eq!(
        live.pending_forced(),
        Some(("zeta eta", 160_000..235_200)),
        "the pending text starts where the settled predecessor ends"
    );
    assert!(state.pending_failure().is_none());
}

#[test]
fn a_forced_window_after_a_sparse_settlement_projects_over_only_the_kept_tail() {
    // The second window decodes 18 words over 2.0 to 20.0 s against 36 over
    // 0 to 10 s, so it is sparse and keeps its last 10 words over 10.0 to
    // 20.0 s. The third window overlaps from 12.0 s, a fifth of the way into
    // that tail, so it settles 2 of the 10 words. Projecting over the second
    // window's whole decode range instead would settle 6.
    let predecessor = numbered_words(36);
    let tail = tail_words("s", 9..=18);
    let third = words("u", 20);
    let state = TakeState::default();
    for outcome in [
        FinalRangeOutcome::forced(ForcedBoundary::first(0..160_000), predecessor.clone()),
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
            words("s", 18),
        ),
    ] {
        state.record_final_outcome(outcome, &[], &ShownHypotheses::default());
    }
    let live = state.live_prefix_snapshot();
    assert_eq!(live.finalized(), predecessor);
    assert_eq!(
        live.pending_forced(),
        Some((tail.as_str(), 160_000..320_000))
    );

    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(192_000..320_000, 320_000..480_000),
            third.clone(),
        ),
        &[],
        &ShownHypotheses::default(),
    );

    assert!(state.pending_failure().is_none());
    assert_eq!(
        state.finalized(),
        format!("{predecessor} {}", tail_words("s", 9..=10)),
        "the third window settles only the kept tail's first fifth"
    );
    assert_eq!(
        state
            .completion(&[], 480_000)
            .expect("the chain of forced windows completes"),
        format!("{predecessor} {} {third}", tail_words("s", 9..=10)),
    );
}

#[test]
fn a_successor_at_exactly_one_third_of_the_predecessor_density_is_not_sparse() {
    // 30 tokens over 0..160_000 against 18 over 32_000..320_000: 18 / 288_000
    // is exactly a third of 30 / 160_000, so projection applies. The overlap
    // starts a fifth of the way in, so 6 predecessor tokens stay.
    let predecessor = numbered_words(30);
    let successor = words("s", 18);

    assert_estimated(
        completion(
            0..160_000,
            32_000..160_000,
            160_000..320_000,
            &predecessor,
            &successor,
        ),
        &format!("{} {successor}", numbered_words(6)),
    );
}

#[test]
fn a_successor_just_below_one_third_of_the_predecessor_density_is_sparse() {
    // 17 tokens project 128_000 / 288_000 of the way to token 8, so the
    // successor keeps its last 9 words after the whole predecessor.
    let predecessor = numbered_words(30);
    let successor = words("s", 17);

    assert_estimated(
        completion(
            0..160_000,
            32_000..160_000,
            160_000..320_000,
            &predecessor,
            &successor,
        ),
        &format!("{predecessor} {}", tail_words("s", 9..=17)),
    );
}

#[test]
fn an_over_limit_final_transcript_fails_before_it_can_become_pending() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(ForcedBoundary::first(0..160_000), "x".repeat(16 * 1024 + 1)),
        &[],
        &ShownHypotheses::default(),
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
