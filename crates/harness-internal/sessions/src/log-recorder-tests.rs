//! `LogRecorder` writes a whole run into the run log in call order, the
//! log reads it back record for record, and a log failure reaches the
//! caller as a `RecorderError` whose source is the log's own error.

use std::error::Error as _;
use std::sync::Arc;

use harness_log::{LogError, RecordFilter, RunLog};
use harness_runner::display_chain;
use harness_runner::recorder::{Record, RecordKind, RunId, RunMeta, RunOutcome, RunRecorder};
use serde_json::json;

use super::LogRecorder;
use crate::runtime::SharedLog;

fn meta() -> RunMeta {
    RunMeta {
        session_id: "session-1".to_owned(),
        agent: "recorder-test".to_owned(),
        prompt_hash: "sha256:fixture".to_owned(),
        seed: u64::MAX,
        flags: 3,
        started_at: 1_700_000_000_000,
    }
}

fn record(kind: RecordKind, task_seq: u32, effect_id: Option<u64>, label: &str) -> Record {
    Record {
        task_id: "0.1".to_owned(),
        task_seq,
        kind,
        effect_id,
        payload: json!({ "label": label, "nested": { "n": [1, 2, 3] } }),
    }
}

async fn recorder() -> (SharedLog, LogRecorder) {
    let log: SharedLog = Arc::new(tokio::sync::Mutex::new(
        RunLog::in_memory().await.expect("the log opens"),
    ));
    let recorder = LogRecorder::new(Arc::clone(&log));
    (log, recorder)
}

#[tokio::test]
async fn a_whole_run_written_through_the_recorder_reads_back_record_for_record() {
    let (log, recorder) = recorder().await;
    let written = [
        record(RecordKind::Event, 0, None, "started"),
        record(RecordKind::Effect, 1, Some(0), "chat issued"),
        record(RecordKind::Answer, 1, Some(0), "chat answered"),
        record(RecordKind::Event, 2, None, "finished"),
    ];
    let outcome = RunOutcome::Completed {
        final_text: "done".to_owned(),
    };

    let run = recorder.begin_run(meta()).await.unwrap();
    for item in &written {
        recorder.append(run, item.clone()).await.unwrap();
    }
    recorder.end_run(run, outcome.clone()).await.unwrap();

    let log = log.lock().await;
    let row = log.run(run).await.unwrap();
    assert_eq!(row.id, run, "the log issued the id the recorder returned");
    assert_eq!(row.meta, meta(), "the run's metadata reads back whole");
    assert_eq!(row.outcome, Some(outcome));
    assert!(row.ended_at.is_some(), "end_run closed the run");

    let stored = log.records(run, RecordFilter::default()).await.unwrap();
    let read: Vec<Record> = stored.into_iter().map(|item| item.record).collect();
    assert_eq!(
        read, written,
        "the records read back whole and in append order"
    );
}

#[tokio::test]
async fn a_log_error_reaches_the_caller_as_a_recorder_error_with_the_log_error_as_source() {
    let (_log, recorder) = recorder().await;
    let run = recorder.begin_run(meta()).await.unwrap();

    let unknown = recorder
        .append(
            RunId::from_raw(99),
            record(RecordKind::Event, 0, None, "lost"),
        )
        .await
        .expect_err("run 99 was never begun");
    let cause = unknown
        .source()
        .and_then(|source| source.downcast_ref::<LogError>())
        .expect("the log's own error is the source");
    assert!(
        matches!(cause, LogError::UnknownRun(id) if id.get() == 99),
        "the source names the unknown run: {cause:?}"
    );

    recorder.end_run(run, RunOutcome::Cancelled).await.unwrap();
    let ended = recorder
        .append(run, record(RecordKind::Event, 0, None, "late"))
        .await
        .expect_err("an ended run takes no more records");
    assert!(
        display_chain(&ended).contains("has ended"),
        "the chain says the run ended: {}",
        display_chain(&ended)
    );
    let again = recorder
        .end_run(run, RunOutcome::Cancelled)
        .await
        .expect_err("a run ends once");
    assert!(
        again
            .source()
            .and_then(|source| source.downcast_ref::<LogError>())
            .is_some_and(|cause| matches!(cause, LogError::RunEnded(id) if *id == run)),
        "a second end_run is refused with the log's RunEnded"
    );
}
