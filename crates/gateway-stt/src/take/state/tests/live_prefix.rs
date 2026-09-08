use super::super::TakeState;
use crate::segment::ForcedBoundary;
use crate::take::final_outcome::FinalRangeOutcome;

#[test]
fn pending_forced_text_is_live_without_becoming_authoritative() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::first(0..160_000),
            "alpha beta ECHO, now".to_owned(),
        ),
        &[],
    );

    let live = state.live_prefix_snapshot();
    assert_eq!(live.finalized(), "");
    assert_eq!(live.finalized_samples(), 0);
    assert_eq!(
        live.pending_forced(),
        Some(("alpha beta ECHO, now", 0..160_000))
    );
    assert_eq!(live.coverage_end(), 160_000);
    assert_eq!(
        state.finalized(),
        "",
        "revisable forced text never enters the final-decode prompt"
    );
}

#[test]
fn fuzzy_reconciliation_atomically_replaces_the_pending_live_range() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::first(0..160_000),
            "alpha beta ECHO, now".to_owned(),
        ),
        &[],
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
            "echo now revised ending".to_owned(),
        ),
        &[],
    );

    let live = state.live_prefix_snapshot();
    assert_eq!(live.finalized(), "alpha beta");
    assert_eq!(live.finalized_samples(), 32_000);
    assert_eq!(
        live.pending_forced(),
        Some(("echo now revised ending", 32_000..320_000))
    );
    assert_eq!(live.coverage_end(), 320_000);
    assert_eq!(
        state.finalized(),
        "alpha beta",
        "only the reconciled old prefix becomes authoritative"
    );
    assert_eq!(
        state
            .completion(&[], 320_000)
            .expect("the replacement pending range completes"),
        "alpha beta echo now revised ending"
    );
}

#[test]
fn estimated_reconciliation_replaces_the_prior_live_prefix() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::first(0..160_000),
            "owned overlap one two three".to_owned(),
        ),
        &[],
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
            "unrelated revision".to_owned(),
        ),
        &[],
    );

    let live = state.live_prefix_snapshot();
    assert_eq!(live.finalized(), "owned");
    assert_eq!(live.finalized_samples(), 32_000);
    assert_eq!(
        live.pending_forced(),
        Some(("unrelated revision", 32_000..320_000))
    );
    assert!(state.pending_failure().is_none());
}
