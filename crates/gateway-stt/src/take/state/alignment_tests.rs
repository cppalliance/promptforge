use super::TakeState;
use crate::segment::ForcedBoundary;
use crate::take::final_outcome::FinalRangeOutcome;

mod adversaries;

#[test]
fn captured_native_outputs_zero_one_two_complete_without_duplicate_phrases() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::first(0..160_000),
            "The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances.".to_owned(),
        ),
        &[],
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
            "Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances. The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances.".to_owned(),
        ),
        &[],
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(192_000..320_000, 320_000..480_000),
            "quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances. The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window".to_owned(),
        ),
        &[],
    );

    assert_eq!(
        state
            .completion(&[], 480_000)
            .expect("the installed sequence reconciles"),
        "The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances. The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window advances. The silver bird circles the quiet garden. Then the silver bird returns beside the river. We continue speaking clearly while the rolling window"
    );
    assert!(state.pending_failure().is_none());
}

#[test]
fn forced_overlap_accepts_bounded_insertions_deletions_and_substitutions() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::first(0..180),
            "settled one, two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen".to_owned(),
        ),
        &[],
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(20..180, 180..200),
            "one two three extra four five SIX seven nine ten eleven dozen thirteen fourteen fifteen fresh".to_owned(),
        ),
        &[],
    );

    assert_eq!(
        state
            .completion(&[], 200)
            .expect("three edits across fifteen tokens reconcile"),
        "settled one two three extra four five SIX seven nine ten eleven dozen thirteen fourteen fifteen fresh"
    );
    assert!(state.pending_failure().is_none());
}
