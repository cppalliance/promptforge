//! The effect loop against fake performers and an in-memory log: the
//! record stream is events, then effects, then answers per step; a cancel
//! drops every outstanding effect with one `Dropped` answer each; a
//! blocking store operation is awaited before the run reaches `Done`; a
//! performer that panics drops its effect rather than stranding the run;
//! and a refused log write ends the drive with the log's error and aborts
//! the performers still out.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness_log::{
    LogError, RecordFilter, RecordKind, RunId, RunLog, RunMeta, RunOutcome, StoredRecord,
};
use harness_runner::effect_loop::{DriveError, SharedLog, drive_run};
use promptforge_api_types::cancel::CancelHandle;
use promptforge_api_types::event::Event;
use serde_json::json;

use crate::support::{
    ClosingInput, PanickingInput, PendingInput, PendingTimer, SlowStore, TIMED_MAIN, TextInput,
    UnitStore, run, run_with_child, unused,
};

/// A run's opening row; the loop closes it.
fn meta() -> RunMeta {
    RunMeta {
        session_id: "session-1".to_owned(),
        agent: "runner-test".to_owned(),
        prompt_hash: "sha256:fixture".to_owned(),
        seed: 7,
        flags: 0,
        started_at: 0,
    }
}

/// An in-memory log with one run begun in it.
async fn begun_log() -> (SharedLog, RunId) {
    let mut log = RunLog::in_memory().await.unwrap();
    let run_id = log.begin_run(meta()).await.unwrap();
    (Arc::new(tokio::sync::Mutex::new(log)), run_id)
}

/// Fires `cancel` from another thread after `delay`: the host's cancel
/// arriving while the loop waits, without a second tokio task in the
/// test (the harness spawns only through its tagged wrapper).
fn cancel_after(cancel: &CancelHandle, delay: Duration) {
    let trigger = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        trigger.cancel();
    });
}

/// Every record of the run, in loop order.
async fn records(log: &SharedLog, run_id: RunId) -> Vec<StoredRecord> {
    log.lock()
        .await
        .records(run_id, RecordFilter::default())
        .await
        .unwrap()
}

/// The kinds of `records`, in order.
fn kinds(records: &[StoredRecord]) -> Vec<RecordKind> {
    records.iter().map(|stored| stored.record.kind).collect()
}

/// Asserts every effect record has exactly one answer record, that the
/// answer comes after its effect, and that the two share one provenance.
fn assert_one_answer_per_effect(records: &[StoredRecord]) {
    let effects: Vec<&StoredRecord> = records
        .iter()
        .filter(|stored| stored.record.kind == RecordKind::Effect)
        .collect();
    let answers: Vec<&StoredRecord> = records
        .iter()
        .filter(|stored| stored.record.kind == RecordKind::Answer)
        .collect();
    assert_eq!(effects.len(), answers.len(), "one answer per effect");
    for effect in effects {
        let id = effect
            .record
            .effect_id
            .expect("an effect record names its id");
        let matching: Vec<&&StoredRecord> = answers
            .iter()
            .filter(|answer| answer.record.effect_id == Some(id))
            .collect();
        assert_eq!(matching.len(), 1, "effect {id} has exactly one answer");
        let answer = matching[0];
        assert!(answer.seq > effect.seq, "the answer follows its effect");
        assert_eq!(answer.record.task_id, effect.record.task_id);
        assert_eq!(answer.record.task_seq, effect.record.task_seq);
    }
}

#[tokio::test]
async fn records_are_events_then_effects_then_answers_per_step() {
    let (log, run_id) = begun_log().await;
    let mut performers = unused();
    performers.store = Arc::new(UnitStore);
    performers.input = Arc::new(TextInput("hi"));
    let seen: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);

    let outcome = drive_run(
        run("store.write('notes.md', 'kept')\nreturn user_input()"),
        performers,
        Arc::clone(&log),
        run_id,
        CancelHandle::new(),
        move |event| sink.lock().unwrap().push(event),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Completed {
            final_text: "hi".to_owned()
        }
    );

    let records = records(&log, run_id).await;
    let kinds = kinds(&records);
    // The run opens with its own events before the first effect; each
    // effect is followed by its answer before the next step's events.
    assert_eq!(kinds[0], RecordKind::Event, "a step's events come first");
    let effect_positions: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| **kind == RecordKind::Effect)
        .map(|(position, _)| position)
        .collect();
    assert_eq!(
        effect_positions.len(),
        2,
        "one store effect, one input effect"
    );
    for position in &effect_positions {
        assert_eq!(
            kinds[position + 1],
            RecordKind::Answer,
            "a serial run's answer follows its effect"
        );
    }
    assert_eq!(
        *kinds.last().unwrap(),
        RecordKind::Event,
        "the run's end is an event"
    );
    assert_one_answer_per_effect(&records);

    let payload = |position: usize| records[position].record.payload.clone();
    assert_eq!(
        payload(effect_positions[0]),
        json!({ "Store": { "op": { "Write": { "path": "notes.md", "contents": "kept" } } } })
    );
    assert_eq!(
        payload(effect_positions[0] + 1),
        json!({ "Store": { "Ok": "Unit" } })
    );
    assert_eq!(
        payload(effect_positions[1]),
        json!({ "UserInput": { "execution": "runner-test", "section": "Only" } })
    );
    assert_eq!(
        payload(effect_positions[1] + 1),
        json!({ "UserInput": { "Ok": { "Text": "hi" } } })
    );

    // Every logged event reached the sink, in order, and the row closed.
    let logged_events: Vec<serde_json::Value> = records
        .iter()
        .filter(|stored| stored.record.kind == RecordKind::Event)
        .map(|stored| stored.record.payload.clone())
        .collect();
    let delivered: Vec<serde_json::Value> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(logged_events, delivered);
    let row = log.lock().await.run(run_id).await.unwrap();
    assert_eq!(row.outcome, Some(outcome));
}

/// The answer records of `records`, in loop order.
fn answers(records: &[StoredRecord]) -> Vec<&StoredRecord> {
    records
        .iter()
        .filter(|stored| stored.record.kind == RecordKind::Answer)
        .collect()
}

/// Waits until `flag` is raised, or fails after a bounded wait: an
/// aborted task is torn down by the runtime after the abort, not at it.
/// Under paused time each sleep is a yield that lets the teardown run
/// and then advances the clock, so the wait costs no wall time.
async fn await_raised(flag: &AtomicBool, what: &str) {
    for _ in 0..200 {
        if flag.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{what} within a second");
}

#[tokio::test]
async fn a_cancel_writes_one_dropped_answer_per_outstanding_effect() {
    let (log, run_id) = begun_log().await;
    let timer_dropped = Arc::new(AtomicBool::new(false));
    let mut performers = unused();
    performers.input = Arc::new(PendingInput);
    performers.timer = Arc::new(PendingTimer {
        dropped: Arc::clone(&timer_dropped),
    });
    let cancel = CancelHandle::new();
    cancel_after(&cancel, Duration::from_millis(50));

    // Two effects are out when the cancel lands: the main section's
    // timeout timer and its child's input wait.
    let outcome = drive_run(
        run_with_child(TIMED_MAIN, "return user_input()"),
        performers,
        Arc::clone(&log),
        run_id,
        cancel,
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(outcome, RunOutcome::Cancelled);

    let records = records(&log, run_id).await;
    assert_one_answer_per_effect(&records);
    let effects: Vec<serde_json::Value> = records
        .iter()
        .filter(|stored| stored.record.kind == RecordKind::Effect)
        .map(|stored| stored.record.payload.clone())
        .collect();
    assert_eq!(
        effects,
        vec![
            json!({ "Timer": { "seconds": 30.0 } }),
            json!({ "UserInput": { "execution": "runner-test", "section": "Child" } }),
        ],
        "the timer and the child's wait are the two effects out"
    );
    let answers = answers(&records);
    assert_eq!(answers.len(), 2, "one drop per outstanding effect");
    for answer in &answers {
        assert_eq!(answer.record.payload, json!("Dropped"));
    }
    assert_ne!(
        answers[0].record.effect_id, answers[1].record.effect_id,
        "each drop answers its own effect"
    );
    assert!(
        timer_dropped.load(Ordering::SeqCst),
        "the parked timer's performer was aborted and joined before the run ended"
    );
    let row = log.lock().await.run(run_id).await.unwrap();
    assert_eq!(row.outcome, Some(RunOutcome::Cancelled));
}

#[tokio::test]
async fn a_panicking_performer_drops_its_effect_instead_of_stranding_the_run() {
    let (log, run_id) = begun_log().await;
    let mut performers = unused();
    performers.input = Arc::new(PanickingInput);

    // No cancel fires: only the lost performer's own drop can end the
    // wait, so a loop that never hears from it hangs here.
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        drive_run(
            run("return user_input()"),
            performers,
            Arc::clone(&log),
            run_id,
            CancelHandle::new(),
            |_event| {},
        ),
    )
    .await
    .expect("a lost performer does not strand the loop")
    .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Cancelled,
        "the chain resumed with the cancelled error of a dropped effect"
    );

    let records = records(&log, run_id).await;
    assert_one_answer_per_effect(&records);
    let answers = answers(&records);
    assert_eq!(answers.len(), 1, "the one panicked input wait");
    assert_eq!(answers[0].record.payload, json!("Dropped"));
    let row = log.lock().await.run(run_id).await.unwrap();
    assert_eq!(row.outcome, Some(RunOutcome::Cancelled));
}

#[tokio::test(start_paused = true)]
async fn a_refused_log_write_returns_the_log_error_and_aborts_the_parked_performers() {
    let (log, run_id) = begun_log().await;
    let timer_dropped = Arc::new(AtomicBool::new(false));
    let mut performers = unused();
    performers.input = Arc::new(ClosingInput {
        log: Arc::clone(&log),
        run_id,
    });
    performers.timer = Arc::new(PendingTimer {
        dropped: Arc::clone(&timer_dropped),
    });

    // The child's input performer closes the run's row before it
    // answers, so recording its answer is the loop's first refused
    // write; the main section's timer is still parked at that moment.
    let error = drive_run(
        run_with_child(TIMED_MAIN, "return user_input()"),
        performers,
        Arc::clone(&log),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .expect_err("a refused write ends the drive");
    assert!(
        matches!(error, DriveError::Log(LogError::RunEnded(id)) if id == run_id),
        "the log's refusal is returned as is: {error:?}"
    );

    let records = records(&log, run_id).await;
    assert_eq!(
        records
            .iter()
            .filter(|stored| stored.record.kind == RecordKind::Effect)
            .count(),
        2,
        "both effects were recorded before the log closed"
    );
    assert!(
        answers(&records).is_empty(),
        "the refused answer was not recorded, and nothing after it"
    );
    await_raised(
        &timer_dropped,
        "the parked timer's performer is aborted when the driver is dropped",
    )
    .await;
}

#[tokio::test]
async fn a_closed_run_refuses_the_first_write_before_any_performer_starts() {
    let (log, run_id) = begun_log().await;
    log.lock()
        .await
        .end_run(run_id, RunOutcome::Cancelled)
        .await
        .unwrap();
    let mut performers = unused();
    performers.input = Arc::new(PendingInput);

    // The run's opening events are the first write; nothing is issued
    // after a refused write, so the unused performers are never reached.
    let error = drive_run(
        run("return user_input()"),
        performers,
        Arc::clone(&log),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .expect_err("a closed run refuses its first write");
    assert!(
        matches!(error, DriveError::Log(LogError::RunEnded(id)) if id == run_id),
        "got {error:?}"
    );
    assert!(records(&log, run_id).await.is_empty());
}

#[tokio::test]
async fn a_slow_store_operation_is_awaited_before_done() {
    let (log, run_id) = begun_log().await;
    let finished = Arc::new(AtomicBool::new(false));
    let mut performers = unused();
    performers.store = Arc::new(SlowStore {
        delay: Duration::from_millis(300),
        finished: Arc::clone(&finished),
    });
    let cancel = CancelHandle::new();
    cancel_after(&cancel, Duration::from_millis(30));

    let outcome = drive_run(
        run("store.write('a.md', 'b')\nreturn 'ok'"),
        performers,
        Arc::clone(&log),
        run_id,
        cancel,
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(
        finished.load(Ordering::SeqCst),
        "the blocking store operation ran to completion before the run ended"
    );

    let records = records(&log, run_id).await;
    assert_one_answer_per_effect(&records);
    let answers: Vec<&StoredRecord> = records
        .iter()
        .filter(|stored| stored.record.kind == RecordKind::Answer)
        .collect();
    assert_eq!(answers.len(), 1);
    assert_eq!(
        answers[0].record.payload,
        json!("Dropped"),
        "the store's late outcome is discarded; its one answer is the drop"
    );
}
