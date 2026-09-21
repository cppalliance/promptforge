//! The runner's own performers under the effect loop: a tokio timer fires
//! after its duration and is torn down by a cancel, a store operation
//! runs through the engine's store facade, and a task-events read returns
//! the task's slice from the run log, narrowed by `last`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use harness_log::{RecordKind, RunLog, RunMeta, RunOutcome};
use harness_runner::effect_loop::{SharedLog, drive_run};
use harness_runner::performers::{BoxFuture, InputPerformer, LogTaskEvents, TokioTimer, VfsStore};
use promptforge_api_runtime::input::{InputError, InputOutcome};
use promptforge_api_types::cancel::CancelHandle;
use serde_json::json;

use crate::support::{PendingInput, TIMED_MAIN, run, run_with_child, unused};

/// A run's opening row.
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
async fn begun_log() -> (SharedLog, harness_log::RunId) {
    let mut log = RunLog::in_memory().await.unwrap();
    let run_id = log.begin_run(meta()).await.unwrap();
    (Arc::new(tokio::sync::Mutex::new(log)), run_id)
}

/// Answers every input wait with `text` once `delay` has passed: the
/// operator who replies, but not before the timer.
struct DelayedInput {
    delay: Duration,
    text: &'static str,
}

impl InputPerformer for DelayedInput {
    fn wait(
        &self,
        _execution: String,
        _section: String,
    ) -> BoxFuture<Result<InputOutcome, InputError>> {
        let delay = self.delay;
        let text = self.text.to_owned();
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Ok(InputOutcome::Text(text))
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
    let (log, run_id) = begun_log().await;
    let mut performers = unused();
    performers.timer = Arc::new(TokioTimer);
    performers.input = Arc::new(DelayedInput {
        delay: Duration::from_millis(400),
        text: "late",
    });

    // The first wait times out at 50ms while the child is still parked on
    // its 400ms input; the second wait, without a timer, delivers it.
    let main = "local t = tasks.spawn('## Child')\n\
        local first = tasks.when_any({ t }, { timeout = 0.05 })\n\
        local _task, ok, result = tasks.when_any({ t })\n\
        return tostring(first == nil) .. '|' .. tostring(ok) .. '|' .. result";
    let outcome = drive_run(
        run_with_child(main, "return user_input()"),
        performers,
        Arc::clone(&log),
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

    // The timer is measured alone: the log stamps the effect row when the
    // effect is issued and the answer row when the sleep returns, so the
    // gap between the two is the sleep and nothing else. The whole run's
    // wall time would not do; the child's 400ms input holds it open
    // regardless of what the timer did.
    let records = log
        .lock()
        .await
        .records(run_id, harness_log::RecordFilter::default())
        .await
        .unwrap();
    let timer = records
        .iter()
        .find(|stored| {
            stored.record.kind == RecordKind::Effect
                && stored.record.payload == json!({ "Timer": { "seconds": 0.05 } })
        })
        .expect("the timed wait issues one timer effect");
    let answer = records
        .iter()
        .find(|stored| {
            stored.record.kind == RecordKind::Answer
                && stored.record.effect_id == timer.record.effect_id
        })
        .expect("the timer effect is answered");
    assert_eq!(
        answer.record.payload,
        json!("Timer"),
        "a fired timer is answered as a timer, not dropped"
    );
    let slept = answer.at - timer.at;
    assert!(
        slept >= 50,
        "the timer was answered no earlier than its duration after it was issued: {slept}ms"
    );
}

#[tokio::test]
async fn a_pending_timer_is_torn_down_by_a_cancel() {
    let (log, run_id) = begun_log().await;
    let mut performers = unused();
    performers.timer = Arc::new(TokioTimer);
    performers.input = Arc::new(PendingInput);
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
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the cancel tore the sleep down instead of waiting it out"
    );

    let records = log
        .lock()
        .await
        .records(run_id, harness_log::RecordFilter::default())
        .await
        .unwrap();
    let dropped = records
        .iter()
        .filter(|stored| {
            stored.record.kind == RecordKind::Answer && stored.record.payload == json!("Dropped")
        })
        .count();
    assert_eq!(
        dropped, 2,
        "the timer and the child's wait are both dropped"
    );
}

#[tokio::test]
async fn the_vfs_store_performs_the_operation_the_effect_names() {
    let (log, run_id) = begun_log().await;
    let mut performers = unused();
    performers.store = Arc::new(VfsStore);

    let outcome = drive_run(
        run("store.write('notes.md', 'kept')\n\
             store.append('notes.md', ' and more')\n\
             local ok, err = pcall(store.read, 'missing.md')\n\
             return store.read('notes.md') .. '|' .. tostring(ok) .. '|' .. tostring(store.exists('notes.md'))"),
        performers,
        Arc::clone(&log),
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

#[tokio::test]
async fn task_events_returns_the_tasks_slice_and_last_narrows_it_to_later_events() {
    let (log, run_id) = begun_log().await;
    let mut performers = unused();
    performers.task_events = Arc::new(LogTaskEvents::new(Arc::clone(&log), run_id));

    // The owner reads the child's whole record, then everything after the
    // first event; every event names the child's task.
    let main = "local t = tasks.spawn('## Child')\n\
        tasks.when_any({ t })\n\
        local all = tasks.events(t)\n\
        local same = true\n\
        for _, e in ipairs(all) do same = same and e.provenance.task == t.task end\n\
        local later = tasks.events(t, { last = all[1].provenance.seq })\n\
        local none = tasks.events(t, { last = all[#all].provenance.seq })\n\
        return all[#all].kind .. '|' .. #all .. '|' .. #later .. '|' .. #none .. '|' .. tostring(same)";
    let outcome = drive_run(
        run_with_child(main, "return 'done'"),
        performers,
        Arc::clone(&log),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    let text = completed(outcome);
    let parts: Vec<&str> = text.split('|').collect();
    assert_eq!(
        parts[0], "task_succeeded",
        "the terminal is the last event of the task's own record: {text}"
    );
    let all: usize = parts[1].parse().expect("a count");
    let later: usize = parts[2].parse().expect("a count");
    let none: usize = parts[3].parse().expect("a count");
    assert!(
        all >= 2,
        "the child reports its chunk and its terminal: {text}"
    );
    assert_eq!(
        later,
        all - 1,
        "`last` drops exactly the events already seen"
    );
    assert_eq!(none, 0, "`last` at the final event reads nothing new");
    assert_eq!(parts[4], "true", "every event names the child's task");
}

#[tokio::test]
async fn task_events_of_a_task_that_never_logged_reads_as_empty() {
    let (log, run_id) = begun_log().await;
    let mut performers = unused();
    performers.task_events = Arc::new(LogTaskEvents::new(Arc::clone(&log), run_id));

    // The main walk reads itself before anything but its own start is
    // logged, and after `last` past every seq it has.
    let outcome = drive_run(
        run("local mine = tasks.events(sys.taskid)\n\
             local none = tasks.events(sys.taskid, { last = 1000000 })\n\
             return tostring(#mine > 0) .. '|' .. #none"),
        performers,
        Arc::clone(&log),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(completed(outcome), "true|0");
}
