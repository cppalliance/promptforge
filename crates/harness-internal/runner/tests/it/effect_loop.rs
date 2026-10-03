//! The effect loop against fake performers and an in-memory recorder: the
//! record stream is events, then effects, then answers per step; a cancel
//! drops every outstanding effect with one `Dropped` answer each; a
//! performer that panics drops its effect rather than stranding the run;
//! and a refused recorder write ends the drive with the recorder's error
//! and tears down the performers still out. The Vfs effect the loop
//! answers inline has its own module, `vfs`, and the `Chat` effect the
//! inference broker answers has `broker`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use harness_runner::display_chain;
use harness_runner::effect_loop::{DriveError, drive_run};
use harness_runner::recorder::{
    MemoryRecorder, Record, RecordKind, RunId, RunMeta, RunOutcome, RunRecorder,
};
use promptforge::cancel::CancelHandle;
use promptforge::vfs::{MemoryBackend, Origin, VfsRef};
use serde_json::json;

use crate::support::{
    ClosingTool, PanickingTool, PendingTimer, PendingTool, TIMED_MAIN, TextTool, WAITS, run,
    run_over, run_with_child, unused,
};

#[path = "effect_loop-broker.rs"]
mod broker;
#[path = "effect_loop-recorder.rs"]
mod recorder_failures;
#[path = "effect_loop-vfs.rs"]
mod vfs;

/// A run's opening metadata; the loop ends the run.
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

/// An in-memory recorder with one run begun in it.
async fn begun_log() -> (Arc<MemoryRecorder>, RunId) {
    let recorder = Arc::new(MemoryRecorder::new());
    let run_id = recorder.begin_run(meta()).await.unwrap();
    (recorder, run_id)
}

/// Fires `cancel` from another thread after `delay`: the Host's cancel
/// arriving while the loop waits, from outside the run's own future.
fn cancel_after(cancel: &CancelHandle, delay: Duration) {
    let trigger = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        trigger.cancel();
    });
}

/// The kinds of `records`, in order.
fn kinds(records: &[Record]) -> Vec<RecordKind> {
    records.iter().map(|record| record.kind).collect()
}

/// Asserts every effect record has exactly one answer record, that the
/// answer comes after its effect, and that the two share one provenance.
fn assert_one_answer_per_effect(records: &[Record]) {
    let effects: Vec<(usize, &Record)> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| record.kind == RecordKind::Effect)
        .collect();
    let answers: Vec<(usize, &Record)> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| record.kind == RecordKind::Answer)
        .collect();
    assert_eq!(effects.len(), answers.len(), "one answer per effect");
    for (position, effect) in effects {
        let id = effect.effect_id.expect("an effect record names its id");
        let matching: Vec<&(usize, &Record)> = answers
            .iter()
            .filter(|(_, answer)| answer.effect_id == Some(id))
            .collect();
        assert_eq!(matching.len(), 1, "effect {id} has exactly one answer");
        let (answer_position, answer) = *matching[0];
        assert!(answer_position > position, "the answer follows its effect");
        assert_eq!(answer.task_id, effect.task_id);
        assert_eq!(answer.task_seq, effect.task_seq);
    }
}

#[tokio::test]
async fn records_are_events_then_effects_then_answers_per_step() {
    let (recorder, run_id) = begun_log().await;
    let mut performers = unused();
    performers.tool = Arc::new(TextTool("hi"));
    let vfs = VfsRef::builder().store("/", MemoryBackend::new()).build();

    let outcome = drive_run(
        run_over(
            &format!("store.write('notes.md', 'kept')\n{WAITS}"),
            vfs.clone(),
        ),
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Completed {
            final_text: "hi".to_owned()
        }
    );
    let store = vfs.acquire_store(Origin::new("effect loop test")).unwrap();
    assert_eq!(
        store.read("notes.md").unwrap(),
        b"kept",
        "the inline answer performed the write on the run's store"
    );

    let records = recorder.records(run_id);
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
    assert_eq!(effect_positions.len(), 2, "one store effect, one tool call");
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

    let payload = |position: usize| records[position].payload.clone();
    assert_eq!(
        payload(effect_positions[0]),
        json!({ "Vfs": { "op": { "Write": { "path": "notes.md", "contents": "kept" } } } })
    );
    assert_eq!(
        payload(effect_positions[0] + 1),
        json!({ "Vfs": { "Ok": "Unit" } })
    );
    assert_eq!(
        payload(effect_positions[1]),
        json!({ "ToolCall": {
            "tool": "tests/runner/wait",
            "alias": "tests/runner/wait",
            "args": {},
            "origin": { "execution": "runner-test", "section": "Only", "caller": "script" }
        } })
    );
    assert_eq!(
        payload(effect_positions[1] + 1),
        json!({ "ToolCall": { "Ok": { "text": "hi", "trusted": true } } })
    );

    // The run ended once, with the loop's outcome.
    assert_eq!(recorder.outcome(run_id), Some(outcome));
}

/// The answer records of `records`, in loop order.
fn answers(records: &[Record]) -> Vec<&Record> {
    records
        .iter()
        .filter(|record| record.kind == RecordKind::Answer)
        .collect()
}

/// Waits until `flag` is raised, or fails after a bounded wait. Under
/// paused time each sleep is a yield that advances the clock, so the wait
/// costs no wall time.
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
    let (recorder, run_id) = begun_log().await;
    let timer_dropped = Arc::new(AtomicBool::new(false));
    let mut performers = unused();
    performers.tool = Arc::new(PendingTool);
    performers.timer = Arc::new(PendingTimer {
        dropped: Arc::clone(&timer_dropped),
    });
    let cancel = CancelHandle::new();
    cancel_after(&cancel, Duration::from_millis(50));

    // Two effects are out when the cancel lands: the main section's
    // timeout timer and its child's tool call.
    let outcome = drive_run(
        run_with_child(TIMED_MAIN, WAITS),
        performers,
        recorder.clone(),
        run_id,
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(outcome, RunOutcome::Cancelled);

    let records = recorder.records(run_id);
    assert_one_answer_per_effect(&records);
    let effects: Vec<serde_json::Value> = records
        .iter()
        .filter(|record| record.kind == RecordKind::Effect)
        .map(|record| record.payload.clone())
        .collect();
    assert_eq!(
        effects,
        vec![
            json!({ "Timer": { "seconds": 30.0 } }),
            json!({ "ToolCall": {
                "tool": "tests/runner/wait",
                "alias": "tests/runner/wait",
                "args": {},
                "origin": { "execution": "runner-test", "section": "Child", "caller": "script" }
            } }),
        ],
        "the timer and the child's tool call are the two effects out"
    );
    let answers = answers(&records);
    assert_eq!(answers.len(), 2, "one drop per outstanding effect");
    for answer in &answers {
        assert_eq!(answer.payload, json!("Dropped"));
    }
    assert_ne!(
        answers[0].effect_id, answers[1].effect_id,
        "each drop answers its own effect"
    );
    assert!(
        timer_dropped.load(Ordering::SeqCst),
        "the parked timer's performer was aborted and joined before the run ended"
    );
    assert_eq!(recorder.outcome(run_id), Some(RunOutcome::Cancelled));
}

#[tokio::test]
async fn a_panicking_performer_drops_its_effect_instead_of_stranding_the_run() {
    let (recorder, run_id) = begun_log().await;
    let mut performers = unused();
    performers.tool = Arc::new(PanickingTool);

    // No cancel fires: only the lost performer's own drop can end the
    // wait, so a loop that never hears from it hangs here.
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        drive_run(
            run(WAITS),
            performers,
            recorder.clone(),
            run_id,
            CancelHandle::new(),
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

    let records = recorder.records(run_id);
    assert_one_answer_per_effect(&records);
    let answers = answers(&records);
    assert_eq!(answers.len(), 1, "the one panicked tool call");
    assert_eq!(answers[0].payload, json!("Dropped"));
    assert_eq!(recorder.outcome(run_id), Some(RunOutcome::Cancelled));
}

#[tokio::test(start_paused = true)]
async fn a_refused_recorder_write_returns_the_recorder_error_and_aborts_the_parked_performers() {
    let (recorder, run_id) = begun_log().await;
    let timer_dropped = Arc::new(AtomicBool::new(false));
    let mut performers = unused();
    performers.tool = Arc::new(ClosingTool {
        recorder: Arc::clone(&recorder),
        run_id,
    });
    performers.timer = Arc::new(PendingTimer {
        dropped: Arc::clone(&timer_dropped),
    });

    // The child's tool performer ends the run at the recorder before it
    // answers, so recording its answer is the loop's first refused write;
    // the main section's timer is still parked at that moment.
    let error = drive_run(
        run_with_child(TIMED_MAIN, WAITS),
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
    )
    .await
    .expect_err("a refused write ends the drive");
    assert!(
        matches!(error, DriveError::Recorder(_)),
        "the recorder's refusal is returned as is: {error:?}"
    );
    assert!(
        display_chain(&error).contains(&format!("run {run_id} has ended")),
        "the chain names the recorder's reason: {}",
        display_chain(&error)
    );

    let records = recorder.records(run_id);
    assert_eq!(
        records
            .iter()
            .filter(|record| record.kind == RecordKind::Effect)
            .count(),
        2,
        "both effects were recorded before the run ended"
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
async fn an_ended_run_refuses_the_first_write_before_any_performer_starts() {
    let (recorder, run_id) = begun_log().await;
    recorder
        .end_run(run_id, RunOutcome::Cancelled)
        .await
        .unwrap();
    let mut performers = unused();
    performers.tool = Arc::new(PendingTool);

    // The run's opening events are the first write; nothing is issued
    // after a refused write, so the unused performers are never reached.
    let error = drive_run(
        run(WAITS),
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
    )
    .await
    .expect_err("an ended run refuses its first write");
    assert!(matches!(error, DriveError::Recorder(_)), "got {error:?}");
    assert!(recorder.records(run_id).is_empty());
}
