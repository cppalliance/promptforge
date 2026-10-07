//! Tests for the normalized, evidence-based, never-shrinking commit rule.

use super::WholeWindowState;

const HALF_SECOND: u64 = 8_000;

fn parts(state: &mut WholeWindowState, end: u64, hypothesis: &str) -> (String, String, String) {
    let (transcript, _, agreed, tentative) = state
        .next("", 0, 0, 0, end, hypothesis)
        .unwrap_or_else(|| panic!("{hypothesis:?} at sample {end} emits"))
        .into_parts();
    (transcript, agreed, tentative)
}

#[test]
fn punctuation_or_case_flip_neither_blocks_nor_reverses_an_agreed_word() {
    let mut state = WholeWindowState::default();
    parts(&mut state, HALF_SECOND, "ask not");

    let (_, agreed, tentative) = parts(&mut state, 2 * HALF_SECOND, "Ask not, what");
    assert_eq!(agreed, "Ask not,", "a case and comma flip still agrees");
    assert_eq!(tentative, " what");

    let (transcript, agreed, _) = parts(&mut state, 3 * HALF_SECOND, "ask not what your");
    assert_eq!(transcript, "Ask not, what your");
    assert_eq!(
        agreed, "Ask not, what",
        "agreed words keep their first text"
    );
}

#[test]
fn agreed_text_never_shrinks_across_a_scripted_sequence() {
    let mut state = WholeWindowState::default();
    let hypotheses = [
        "Why is it",
        "Why is it",
        "Why is this",
        "why, is this the",
        "Why is that the way",
        "Why was it the way?",
        "Why is it the way we",
        "Why is it the way we",
    ];
    let mut previous = String::new();
    for (step, hypothesis) in (1..).zip(hypotheses) {
        let (_, agreed, _) = parts(&mut state, step * HALF_SECOND, hypothesis);
        let words = agreed.split_whitespace().collect::<Vec<_>>();
        assert!(
            words.starts_with(&previous.split_whitespace().collect::<Vec<_>>()),
            "agreed {agreed:?} after {hypothesis:?} does not extend {previous:?}"
        );
        previous = agreed;
    }
    assert_eq!(previous, "Why is it the way we");
}

#[test]
fn disputed_agreed_words_keep_their_text_and_the_hypothesis_continues_after_them() {
    let mut state = WholeWindowState::default();
    parts(&mut state, HALF_SECOND, "And so my fellow");
    parts(&mut state, 2 * HALF_SECOND, "And so my fellow");

    assert_eq!(
        parts(&mut state, 3 * HALF_SECOND, "And so am I fellow Americans."),
        (
            "And so my fellow Americans.".to_owned(),
            "And so my fellow".to_owned(),
            " Americans.".to_owned(),
        )
    );
}

#[test]
fn a_persistent_dispute_still_lets_the_following_words_promote() {
    let mut state = WholeWindowState::default();
    parts(&mut state, HALF_SECOND, "Why is it");
    parts(&mut state, 2 * HALF_SECOND, "Why is it");
    parts(&mut state, 3 * HALF_SECOND, "Why is this the");

    assert_eq!(
        parts(&mut state, 4 * HALF_SECOND, "Why is that the way"),
        (
            "Why is it the way".to_owned(),
            "Why is it the".to_owned(),
            " way".to_owned(),
        ),
        "words after a disputed agreed word gather evidence"
    );
    assert_eq!(
        parts(&mut state, 5 * HALF_SECOND, "Why is that the way we").1,
        "Why is it the way"
    );
}

#[test]
fn a_punctuated_last_word_waits_for_a_later_hypothesis_to_continue_past_it() {
    let mut state = WholeWindowState::default();
    parts(&mut state, HALF_SECOND, "And so my fellow");
    parts(&mut state, 2 * HALF_SECOND, "And so my fellow");
    parts(&mut state, 3 * HALF_SECOND, "And so am I fellow Americans.");

    assert_eq!(
        parts(&mut state, 4 * HALF_SECOND, "And so am I fellow Americans.").1,
        "And so my fellow",
        "two observations half a second apart do not promote a punctuated edge word"
    );
    assert_eq!(
        parts(&mut state, 5 * HALF_SECOND, "And so my fellow Americans").1,
        "And so my fellow",
        "ending on the same word again does not continue past it"
    );
    assert_eq!(
        parts(
            &mut state,
            6 * HALF_SECOND,
            "And so my fellow Americans ask"
        ),
        (
            "And so my fellow Americans ask".to_owned(),
            "And so my fellow Americans".to_owned(),
            " ask".to_owned(),
        ),
        "a continuing hypothesis promotes the word with its own text"
    );
}

#[test]
fn promotion_needs_two_observations_half_a_second_apart() {
    let mut state = WholeWindowState::default();
    let first = 2 * HALF_SECOND;

    assert_eq!(parts(&mut state, first, "alpha beta").1, "");
    assert_eq!(
        parts(&mut state, first + HALF_SECOND - 1, "alpha beta gamma").1,
        "",
        "a second observation one sample short of half a second does not promote"
    );
    assert_eq!(
        parts(&mut state, first + HALF_SECOND, "alpha beta gamma").1,
        "alpha beta",
        "observations half a second apart promote"
    );
}

#[test]
fn outlier_is_held_then_adopted_when_the_next_hypothesis_is_similar() {
    let mut state = WholeWindowState::default();
    parts(&mut state, HALF_SECOND, "red green blue");
    parts(&mut state, 2 * HALF_SECOND, "red green blue");

    assert!(
        state
            .next("", 0, 0, 0, 3 * HALF_SECOND, "one two three four")
            .is_none(),
        "a hypothesis unlike agreed text and recent hypotheses is held"
    );
    assert_eq!(
        parts(&mut state, 4 * HALF_SECOND, "one two three four five"),
        (
            "red green blue four five".to_owned(),
            "red green blue".to_owned(),
            " four five".to_owned(),
        ),
        "a similar follower adopts the held hypothesis and agreed text stands"
    );
}

#[test]
fn outlier_is_dropped_when_the_next_hypothesis_is_not_similar() {
    let mut state = WholeWindowState::default();
    parts(&mut state, HALF_SECOND, "red green blue");
    parts(&mut state, 2 * HALF_SECOND, "red green blue");
    assert!(
        state
            .next("", 0, 0, 0, 3 * HALF_SECOND, "one two three four")
            .is_none()
    );

    parts(&mut state, 4 * HALF_SECOND, "red green blue cyan");
    assert!(
        state
            .next("", 0, 0, 0, 5 * HALF_SECOND, "one two three four five")
            .is_none(),
        "the dropped hypothesis never joined the history, so its echo is held"
    );
}

/// Whether a dispute of agreed "red" is held when the only hypothesis it
/// resembles was followed by `followers` hypotheses of "red".
fn dispute_is_held_after(followers: u64) -> bool {
    let mut state = WholeWindowState::default();
    parts(&mut state, HALF_SECOND, "red green blue cyan magenta");
    for step in 2..=followers + 1 {
        parts(&mut state, step * HALF_SECOND, "red");
    }
    state
        .next(
            "",
            0,
            0,
            0,
            (followers + 2) * HALF_SECOND,
            "green blue cyan magenta",
        )
        .is_none()
}

#[test]
fn a_dispute_similar_only_to_the_sixth_most_recent_hypothesis_is_held() {
    assert!(
        !dispute_is_held_after(4),
        "the fifth-most-recent hypothesis still vouches for the dispute"
    );
    assert!(
        dispute_is_held_after(5),
        "the sixth-most-recent hypothesis has left the history"
    );
}
