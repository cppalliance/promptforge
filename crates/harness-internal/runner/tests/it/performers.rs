//! The runner's own performer and the inline Vfs answer under the effect
//! loop: a tokio timer fires after its duration and is torn down by a
//! cancel, a store operation runs through the Engine's store facade.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness_runner::effect_loop::drive_run;
use harness_runner::performers::{BoxFuture, TokioTimer, ToolPerformer};
use harness_runner::recorder::{
    MemoryRecorder, Record, RecordKind, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder,
};
use promptforge::cancel::CancelHandle;
use promptforge::tools::{ToolError, ToolId, ToolOutput};
use serde_json::{Value, json};

use crate::support::{PendingTool, TIMED_MAIN, WAITS, run, run_with_child, unused};

/// A run's opening metadata.
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

/// A memory recorder that notes when each append arrived, so a test can
/// measure the time between two records. It holds one run: the arrival
/// at index `n` is the `n`th record's.
struct StampedRecorder {
    inner: MemoryRecorder,
    arrivals: Mutex<Vec<Instant>>,
}

impl StampedRecorder {
    fn new() -> Self {
        Self {
            inner: MemoryRecorder::new(),
            arrivals: Mutex::new(Vec::new()),
        }
    }

    fn arrival(&self, position: usize) -> Instant {
        self.arrivals.lock().unwrap()[position]
    }
}

impl RunRecorder for StampedRecorder {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        self.inner.begin_run(meta)
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            self.inner.append(run, record).await?;
            self.arrivals.lock().unwrap().push(Instant::now());
            Ok(())
        })
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        self.inner.end_run(run, outcome)
    }
}

/// Answers every tool call with `text` once `delay` has passed: the tool
/// that answers, but not before the timer.
struct DelayedTool {
    delay: Duration,
    text: &'static str,
}

impl ToolPerformer for DelayedTool {
    fn call(
        &self,
        _tool: ToolId,
        _alias: String,
        _args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>> {
        let delay = self.delay;
        let text = self.text;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(ToolOutput::trusted(text))
        })
    }
}

/// The final text of a completed run.
fn completed(outcome: RunOutcome) -> String {
    match outcome {
        RunOutcome::Completed { final_text } => final_text,
        other => panic!("the run completes: {other:?}"),
    }
}

#[tokio::test]
async fn a_timer_effect_is_answered_after_its_duration() {
    let recorder = Arc::new(StampedRecorder::new());
    let run_id = recorder.begin_run(meta()).await.unwrap();
    let mut performers = unused();
    performers.timer = Arc::new(TokioTimer);
    performers.tool = Arc::new(DelayedTool {
        delay: Duration::from_millis(400),
        text: "late",
    });

    // The first wait times out at 50ms while the child is still parked on
    // its 400ms tool call; the second wait, without a timer, delivers it.
    let main = "local t = tasks.spawn('## Child')\n\
        local first = tasks.join_any({ t }, { timeout = 0.05 })\n\
        local _task, ok, result = tasks.join_any({ t })\n\
        return tostring(first == nil) .. '|' .. tostring(ok) .. '|' .. result";
    let outcome = drive_run(
        run_with_child(main, WAITS),
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(
        completed(outcome),
        "true|true|late",
        "the timed wait returns nil when the timer fires first, and the plain wait \
         then delivers the child"
    );

    // The timer is measured alone: the recorder notes when the effect
    // record arrived and when the answer record did, so the gap between
    // the two is the sleep and nothing else. The whole run's wall time
    // would not do; the child's 400ms tool call holds it open regardless
    // of what the timer did.
    let records = recorder.inner.records(run_id);
    let timer = records
        .iter()
        .position(|record| {
            record.kind == RecordKind::Effect
                && record.payload == json!({ "Timer": { "seconds": 0.05 } })
        })
        .expect("the timed wait issues one timer effect");
    let answer = records
        .iter()
        .position(|record| {
            record.kind == RecordKind::Answer && record.effect_id == records[timer].effect_id
        })
        .expect("the timer effect is answered");
    assert_eq!(
        records[answer].payload,
        json!("Timer"),
        "a fired timer is answered as a timer, not dropped"
    );
    let slept = recorder.arrival(answer) - recorder.arrival(timer);
    assert!(
        slept >= Duration::from_millis(50),
        "the timer was answered no earlier than its duration after it was issued: {slept:?}"
    );
}

#[tokio::test]
async fn a_pending_timer_is_torn_down_by_a_cancel() {
    let (recorder, run_id) = begun_log().await;
    let mut performers = unused();
    performers.timer = Arc::new(TokioTimer);
    performers.tool = Arc::new(PendingTool);
    let cancel = CancelHandle::new();
    let trigger = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        trigger.cancel();
    });

    // The main section's 30-second timer is out when the cancel lands; the
    // run must end at the cancel, not when the timer would have fired.
    let started = Instant::now();
    let outcome = drive_run(
        run_with_child(TIMED_MAIN, WAITS),
        performers,
        recorder.clone(),
        run_id,
        cancel,
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the cancel tore the sleep down instead of waiting it out"
    );

    let dropped = recorder
        .records(run_id)
        .iter()
        .filter(|record| record.kind == RecordKind::Answer && record.payload == json!("Dropped"))
        .count();
    assert_eq!(
        dropped, 2,
        "the timer and the child's tool call are both dropped"
    );
}

#[tokio::test]
async fn an_inline_vfs_answer_performs_the_operation_the_effect_names() {
    let (recorder, run_id) = begun_log().await;
    let performers = unused();

    let outcome = drive_run(
        run("store.write('notes.md', 'kept')\n\
             store.append('notes.md', ' and more')\n\
             local ok, err = pcall(store.read, 'missing.md')\n\
             return store.read('notes.md') .. '|' .. tostring(ok) .. '|' .. tostring(store.exists('notes.md'))"),
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(
        completed(outcome),
        "kept and more|false|true",
        "writes land, reads see them, and a missing path is the store's own failure"
    );
}
