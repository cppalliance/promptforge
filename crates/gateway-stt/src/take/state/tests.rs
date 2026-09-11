use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use gateway_stt_engine::{EnginePolicy, TranscribeError};

use super::{TakeFailure, TakeState};
use crate::segment::ForcedBoundary;
use crate::take::final_outcome::{FinalRangeOutcome, SkipReason};
use crate::take::window::AcceptedHypothesis;

mod live_prefix;

#[test]
fn finalized_snapshot_cannot_mix_text_and_sample_ownership() {
    let state = Arc::new(TakeState::default());
    state.record_finalized(Ok::<_, TranscribeError>("old".to_owned()), Some(100));
    let writer_state = Arc::clone(&state);
    let (writer_ready, ready) = mpsc::channel();
    let (allow_write, write_allowed) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        writer_ready
            .send(())
            .expect("snapshot knows the writer is ready");
        write_allowed
            .recv_timeout(Duration::from_secs(1))
            .expect("snapshot allows the writer to attempt finalization");
        writer_state.record_finalized(Ok::<_, TranscribeError>("new".to_owned()), Some(200));
    });

    let snapshot = state.finalized_snapshot_with(|| {
        ready
            .recv_timeout(Duration::from_secs(1))
            .expect("writer reaches the synchronized snapshot boundary");
        assert!(
            state.finalized.try_lock().is_err(),
            "the text and sample watermark share one held lock"
        );
        allow_write
            .send(())
            .expect("writer waits for the finalized lock");
    });
    writer.join().expect("finalization writer joins");

    assert_eq!(snapshot, ("old".to_owned(), 100));
    assert_eq!(state.finalized_snapshot(), ("old new".to_owned(), 200));
}

#[test]
fn a_recorded_decode_failure_keeps_its_typed_source() {
    let state = TakeState::default();
    let source = EnginePolicy::new(0, 500, false).expect_err("a zero window is rejected");
    let expected = source.to_string();
    state.record_finalized(Err(source), None);

    let failure = state.take_failure().expect("the take owns its failure");
    assert!(
        matches!(&*failure, TakeFailure::Transcribe(_)),
        "the decode failure stays typed"
    );
    assert_eq!(failure.to_string(), expected);
}

#[test]
fn natural_speech_and_pause_cycles_settle_without_history_growth() {
    let state = TakeState::default();
    for index in 0..(4_096 * 2 + 17) {
        let start = u64::try_from(index).expect("test index fits") * 2;
        let decoded_end = start.saturating_add(1);
        state.record_final_outcome(
            FinalRangeOutcome::decoded(start..decoded_end, format!("word{index}")),
            &[],
        );
        state.record_final_outcome(
            FinalRangeOutcome::skipped(
                decoded_end..decoded_end.saturating_add(1),
                SkipReason::Silence,
            ),
            &[],
        );
        assert!(state.pending_failure().is_none());
        assert_eq!(state.coverage().2, 0);
    }
    assert_eq!(state.coverage().0, (4_096 * 2 + 17) as u64 * 2);
}

#[test]
fn unresolved_skip_consumes_one_candidate_then_later_decode_settles_normally() {
    let state = TakeState::default();
    let accepted = [AcceptedHypothesis::new(0..2, "accepted".to_owned())];
    state.record_final_outcome(
        FinalRangeOutcome::skipped(0..1, SkipReason::BelowFinalWindow),
        &accepted,
    );
    assert_eq!(state.coverage(), (0, None, 1));

    state.record_final_outcome(
        FinalRangeOutcome::skipped(1..2, SkipReason::Silence),
        &accepted,
    );
    assert_eq!(state.coverage(), (2, None, 0));
    state.record_final_outcome(
        FinalRangeOutcome::decoded(2..3, "decoded".to_owned()),
        &accepted,
    );

    assert_eq!(
        state.finalized_snapshot(),
        ("accepted decoded".to_owned(), 3)
    );
    assert!(state.pending_failure().is_none());
}

#[test]
fn forced_overlap_freezes_only_the_reconciled_old_prefix() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::first(0..160_000),
            "alpha beta ECHO, now".to_owned(),
        ),
        &[],
    );
    assert_eq!(state.finalized_snapshot(), (String::new(), 0));

    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
            "echo now revised ending".to_owned(),
        ),
        &[],
    );
    assert_eq!(
        state.finalized_snapshot(),
        ("alpha beta".to_owned(), 32_000)
    );

    state.record_final_outcome(
        FinalRangeOutcome::decoded(320_000..336_000, "tail".to_owned()),
        &[],
    );
    assert_eq!(
        state.finalized_snapshot(),
        (
            "alpha beta echo now revised ending tail".to_owned(),
            336_000
        )
    );
}

#[test]
fn forced_overlap_preserves_repeated_phrases_at_distinct_ranges() {
    let state = TakeState::default();
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::first(0..160_000),
            "echo now echo now".to_owned(),
        ),
        &[],
    );
    state.record_final_outcome(
        FinalRangeOutcome::forced(
            ForcedBoundary::overlapping(32_000..160_000, 160_000..320_000),
            "echo now corrected".to_owned(),
        ),
        &[],
    );
    state.record_final_outcome(
        FinalRangeOutcome::skipped(320_000..320_000, SkipReason::BelowFinalWindow),
        &[],
    );

    assert_eq!(state.finalized(), "echo now echo now corrected");
    assert!(state.pending_failure().is_none());
}

#[test]
fn five_unaligned_forced_windows_keep_order_and_flush_the_last_once() {
    let state = TakeState::default();
    let windows = (0..6)
        .map(|window| {
            (0..if window == 0 { 10 } else { 18 })
                .map(|token| format!("w{window}t{token}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>();
    state.record_final_outcome(
        FinalRangeOutcome::forced(ForcedBoundary::first(0..160_000), windows[0].clone()),
        &[],
    );
    for (index, text) in windows.iter().enumerate().skip(1) {
        let overlap_end = u64::try_from(index).expect("window index fits") * 160_000;
        state.record_final_outcome(
            FinalRangeOutcome::forced(
                ForcedBoundary::overlapping(
                    overlap_end - 128_000..overlap_end,
                    overlap_end..overlap_end + 160_000,
                ),
                text.clone(),
            ),
            &[],
        );
        assert!(state.pending_failure().is_none());
        assert_eq!(
            state
                .live_prefix_snapshot()
                .pending_forced()
                .map(|(_, range)| range.end),
            Some(overlap_end + 160_000),
            "window {index} remains the complete live pending range"
        );
    }

    let mut expected = windows[0]
        .split_whitespace()
        .take(2)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for window in &windows[1..5] {
        expected.extend(window.split_whitespace().take(10).map(str::to_owned));
    }
    expected.extend(windows[5].split_whitespace().map(str::to_owned));
    assert_eq!(
        state
            .completion(&[], 960_000)
            .expect("estimated reconciliation remains healthy"),
        expected.join(" ")
    );
    assert!(state.pending_failure().is_none());
}
