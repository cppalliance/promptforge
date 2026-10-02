//! The close path of the session runtime: a close drains the run -
//! outstanding effects are answered `Dropped` before the session is
//! `Closed`; and a requested close reports its synthetic terminal to
//! `subscribe_errors` as one `Interrupted` failure with the frame's wording.

use harness_capabilities::USER_INPUT_ASK_TOOL;
use harness_runner::recorder::{RecordKind, RunOutcome};
use harness_sessions::input::WaitFrame;
use harness_sessions::session::{FailureKind, SessionFailure};
use harness_sessions::transition::{
    EffectiveInterrupt, Interrupt, SessionState, effective_interrupt,
};
use tokio::sync::broadcast;

use super::{PATIENCE, harness, launch, recorded_harness, required_token, wait_for};

#[tokio::test]
async fn closing_answers_outstanding_effects_dropped_before_closed() {
    let dir = tempfile::tempdir().unwrap();
    let (harness, recorder) = recorded_harness(dir.path());
    let session = launch(&harness).await;
    let mut waits = session.subscribe_waits();
    let token = required_token(&mut waits).await;
    assert_eq!(session.state(), SessionState::Alive);
    assert_eq!(session.unresolved_waits(), vec![token.clone()]);

    assert!(harness.close(session.id()), "the session was registered");
    assert_eq!(
        session.state(),
        SessionState::Closing,
        "close is requested: the run is not yet done"
    );
    assert!(
        harness.session(session.id()).is_none(),
        "a closed session leaves the harness at once"
    );

    wait_for(&session, SessionState::Closed).await;

    // The outstanding input wait died as an outcome, not silence.
    assert!(session.unresolved_waits().is_empty(), "no wait leaks");
    let frame = waits.recv().await.expect("the cancelled frame arrives");
    assert_eq!(frame, WaitFrame::Cancelled { token });

    // At the recorder: the wait's effect has exactly one answer,
    // `Dropped`, and the run ended as cancelled - both before `Closed`.
    let runs = session.run_ids();
    assert_eq!(runs.len(), 1);
    assert_eq!(recorder.outcome(runs[0]), Some(RunOutcome::Cancelled));
    let records = recorder.records(runs[0]);
    let position = |wanted: RecordKind| {
        records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.kind == wanted)
            .collect::<Vec<_>>()
    };
    let effects = position(RecordKind::Effect);
    assert_eq!(effects.len(), 1, "one effect was out: the input wait");
    assert_eq!(
        effects[0].1.payload["ToolCall"]["tool"],
        serde_json::json!(USER_INPUT_ASK_TOOL),
        "the input wait is the ask tool's call: {}",
        effects[0].1.payload
    );
    let answers = position(RecordKind::Answer);
    assert_eq!(answers.len(), 1, "every effect has exactly one answer");
    assert_eq!(answers[0].1.effect_id, effects[0].1.effect_id);
    assert_eq!(
        answers[0].1.payload,
        serde_json::json!("Dropped"),
        "the outstanding effect was answered Dropped: {}",
        answers[0].1.payload
    );
    assert!(
        answers[0].0 > effects[0].0,
        "the answer follows the effect it drops"
    );
    assert!(
        !session.transcript(0).is_empty(),
        "the transcript stays readable after close"
    );
}

#[tokio::test]
async fn a_requested_close_reports_one_interrupted_failure_by_kind() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let session = launch(&harness).await;
    let mut errors = session.subscribe_errors();
    let mut waits = session.subscribe_waits();
    let _token = required_token(&mut waits).await;

    // Parking on input is not a failure: nothing has been reported yet.
    assert!(
        matches!(
            errors.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ),
        "a parked run reports no failure"
    );

    assert!(harness.close(session.id()), "the session was registered");
    wait_for(&session, SessionState::Closed).await;

    // The close interrupted a run that saw no genuine terminal, so the
    // supervisor renders the interrupt's frame once, after the drain, as
    // a failure whose kind is the machine-readable fact and whose message
    // is the frame's wording.
    let EffectiveInterrupt::Terminal(frame) = effective_interrupt(Interrupt::Cancel, false) else {
        panic!("a cancel before any terminal is the run's terminal");
    };
    let failure = tokio::time::timeout(PATIENCE, errors.recv())
        .await
        .expect("the interrupt's failure is reported in time")
        .expect("the failure report arrives");
    assert_eq!(
        failure,
        SessionFailure {
            kind: FailureKind::Interrupted,
            message: frame.message().to_owned(),
        },
        "the close is reported as Interrupted, not as a failed run"
    );
    assert!(
        matches!(
            errors.try_recv(),
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed)
        ),
        "the interrupt renders exactly one failure"
    );
}
