//! The close path of the session runtime: a close drains the run -
//! outstanding effects are answered `Dropped` before the session is
//! `Closed`; and a requested close reports its synthetic terminal to
//! `subscribe_errors` as one `Interrupted` failure carrying the frame's
//! wording.

use harness_log::{RecordKind, RunOutcome};
use harness_sessions::input::WaitFrame;
use harness_sessions::session::{FailureKind, SessionFailure};
use harness_sessions::transition::{
    EffectiveInterrupt, Interrupt, SessionState, effective_interrupt,
};
use tokio::sync::broadcast;

use super::{PATIENCE, harness, launch, required_token, wait_for};

#[tokio::test]
async fn closing_answers_outstanding_effects_dropped_before_closed() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
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

    // In the log: the wait's effect has exactly one answer, `Dropped`,
    // and the run's row closed as cancelled - both before `Closed`.
    let log = harness.log().await.unwrap();
    let runs = session.run_ids();
    assert_eq!(runs.len(), 1);
    let log = log.lock().await;
    let row = log.run(runs[0]).await.unwrap();
    assert_eq!(row.outcome, Some(RunOutcome::Cancelled));
    let records = log
        .records(runs[0], harness_log::RecordFilter::default())
        .await
        .unwrap();
    let effects: Vec<_> = records
        .iter()
        .filter(|stored| stored.record.kind == RecordKind::Effect)
        .collect();
    assert_eq!(effects.len(), 1, "one effect was out: the input wait");
    let answers: Vec<_> = records
        .iter()
        .filter(|stored| stored.record.kind == RecordKind::Answer)
        .collect();
    assert_eq!(answers.len(), 1, "every effect has exactly one answer");
    assert_eq!(answers[0].record.effect_id, effects[0].record.effect_id);
    assert_eq!(
        answers[0].record.payload,
        serde_json::json!("Dropped"),
        "the outstanding effect was answered Dropped: {}",
        answers[0].record.payload
    );
    assert!(
        answers[0].seq > effects[0].seq,
        "the answer follows the effect it drops"
    );
    drop(log);
    assert!(
        matches!(session.transcript(0).await, Ok(events) if !events.is_empty()),
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
