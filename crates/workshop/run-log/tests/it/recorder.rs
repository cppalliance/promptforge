//! `TursoRecorder`: a whole run goes in through the Harness's recorder
//! trait and comes back out through `RunLog`.

// clippy.toml's allow-unwrap-in-tests covers #[test] functions only, not
// the helpers they share; failing a test by panicking is what these are for.
#![expect(
    clippy::unwrap_used,
    reason = "test helpers fail by panicking with the failure shown"
)]

use std::error::Error as _;
use std::fs;
use std::path::Path;

use harness::record::{Record, RecordKind, RecorderError, RunId, RunMeta, RunOutcome, RunRecorder};
use serde_json::json;
use workshop_run_log::{LogError, RecordFilter, RunLog, TursoRecorder};

/// Every record of a run, in loop order.
const ALL: RecordFilter = RecordFilter {
    kind: None,
    last: None,
};

/// A run's opening row as the Harness would write it.
fn meta(session_id: &str) -> RunMeta {
    RunMeta {
        session_id: session_id.to_owned(),
        agent: "chat".to_owned(),
        prompt_hash: "sha256:abc".to_owned(),
        seed: u64::MAX - 1,
        flags: 0,
        started_at: 1_700_000_000_000,
    }
}

/// One record of `kind` on task 0 at `task_seq`, tagged with `owner` so a
/// read can tell whose it is.
fn record(kind: RecordKind, task_seq: u32, effect_id: Option<u64>, owner: &str) -> Record {
    Record {
        task_id: "0".to_owned(),
        task_seq,
        kind,
        effect_id,
        payload: json!({ "owner": owner, "task_seq": task_seq }),
    }
}

/// Opens the file a recorder wrote, once the recorder is gone.
async fn read_back(path: &Path) -> RunLog {
    RunLog::open(path).await.unwrap()
}

/// The log failure behind a recorder error.
fn log_failure(error: &RecorderError) -> &LogError {
    let Some(cause) = error.source() else {
        panic!("a recorder error carries its cause as source()");
    };
    let Some(failure) = cause.downcast_ref::<LogError>() else {
        panic!("the cause is the run log's own error: {cause}");
    };
    failure
}

#[tokio::test]
async fn a_whole_run_goes_in_through_the_recorder_and_reads_back_through_the_log() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("harness");
    let recorder = TursoRecorder::new(&state);
    let path = recorder.path().to_path_buf();
    assert_eq!(path.parent(), Some(state.as_path()));
    assert!(
        !state.exists(),
        "building the recorder touches no file; the first run does"
    );

    let run = recorder.begin_run(meta("session-1")).await.unwrap();
    recorder
        .append(run, record(RecordKind::Effect, 0, Some(7), "a"))
        .await
        .unwrap();
    recorder
        .append(run, record(RecordKind::Answer, 1, Some(7), "a"))
        .await
        .unwrap();
    recorder
        .append(run, record(RecordKind::Event, 2, None, "a"))
        .await
        .unwrap();
    let outcome = RunOutcome::Completed {
        final_text: "done".to_owned(),
    };
    recorder.end_run(run, outcome.clone()).await.unwrap();
    drop(recorder);

    let log = read_back(&path).await;
    let row = log.run(run).await.unwrap();
    assert_eq!(row.meta, meta("session-1"));
    assert_eq!(row.outcome, Some(outcome));
    assert!(row.ended_at.is_some());

    let records = log.records(run, ALL).await.unwrap();
    let stored: Vec<(RecordKind, u32, Option<u64>)> = records
        .iter()
        .map(|stored| {
            (
                stored.record.kind,
                stored.record.task_seq,
                stored.record.effect_id,
            )
        })
        .collect();
    assert_eq!(
        stored,
        [
            (RecordKind::Effect, 0, Some(7)),
            (RecordKind::Answer, 1, Some(7)),
            (RecordKind::Event, 2, None),
        ]
    );
    for (index, stored) in records.iter().enumerate() {
        let task_seq = u32::try_from(index).unwrap();
        assert_eq!(
            stored.record.payload,
            json!({ "owner": "a", "task_seq": task_seq })
        );
    }
}

#[tokio::test]
async fn a_failed_open_is_retried_by_the_next_call() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("harness");
    fs::write(&state, b"a file where the state directory belongs").unwrap();
    let recorder = TursoRecorder::new(&state);

    let error = recorder.begin_run(meta("session-1")).await.unwrap_err();
    assert!(
        matches!(log_failure(&error), LogError::Io { .. }),
        "the directory cannot be created: {error:?}"
    );

    fs::remove_file(&state).unwrap();
    let run = recorder.begin_run(meta("session-1")).await.unwrap();
    recorder
        .append(run, record(RecordKind::Event, 0, None, "a"))
        .await
        .unwrap();
    recorder.end_run(run, RunOutcome::Cancelled).await.unwrap();
    let path = recorder.path().to_path_buf();
    drop(recorder);

    let log = read_back(&path).await;
    assert_eq!(
        log.run(run).await.unwrap().outcome,
        Some(RunOutcome::Cancelled)
    );
    assert_eq!(log.records(run, ALL).await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_same_file_reopens_with_its_rows() {
    let dir = tempfile::TempDir::new().unwrap();
    let state = dir.path().join("harness");

    let first = TursoRecorder::new(&state);
    let earlier = first.begin_run(meta("session-1")).await.unwrap();
    first
        .append(earlier, record(RecordKind::Event, 0, None, "first"))
        .await
        .unwrap();
    first.end_run(earlier, RunOutcome::Cancelled).await.unwrap();
    drop(first);

    let second = TursoRecorder::new(&state);
    let later = second.begin_run(meta("session-2")).await.unwrap();
    assert_ne!(later, earlier, "a reopened file issues a fresh run id");
    let path = second.path().to_path_buf();
    drop(second);

    let log = read_back(&path).await;
    let kept = log.run(earlier).await.unwrap();
    assert_eq!(kept.meta.session_id, "session-1");
    assert_eq!(kept.outcome, Some(RunOutcome::Cancelled));
    assert_eq!(log.records(earlier, ALL).await.unwrap().len(), 1);
    let open = log.run(later).await.unwrap();
    assert_eq!(open.meta.session_id, "session-2");
    assert_eq!(open.outcome, None);
}

#[tokio::test]
async fn a_write_the_log_refuses_reaches_the_caller_as_a_recorder_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let recorder = TursoRecorder::new(dir.path().join("harness"));

    let unknown = RunId::from_raw(41);
    let error = recorder
        .append(unknown, record(RecordKind::Event, 0, None, "a"))
        .await
        .unwrap_err();
    assert!(
        matches!(log_failure(&error), LogError::UnknownRun(run) if *run == unknown),
        "{error:?}"
    );

    let run = recorder.begin_run(meta("session-1")).await.unwrap();
    recorder.end_run(run, RunOutcome::Cancelled).await.unwrap();
    let error = recorder
        .end_run(run, RunOutcome::Cancelled)
        .await
        .unwrap_err();
    assert!(
        matches!(log_failure(&error), LogError::RunEnded(ended) if *ended == run),
        "{error:?}"
    );
}

/// Begins a run for `owner`, appends three events, and ends it.
async fn whole_run(recorder: &TursoRecorder, owner: &str) -> RunId {
    let run = recorder.begin_run(meta(owner)).await.unwrap();
    for task_seq in 0..3 {
        recorder
            .append(run, record(RecordKind::Event, task_seq, None, owner))
            .await
            .unwrap();
    }
    recorder.end_run(run, RunOutcome::Cancelled).await.unwrap();
    run
}

#[tokio::test]
async fn runs_that_overlap_keep_their_own_records() {
    let dir = tempfile::TempDir::new().unwrap();
    let recorder = TursoRecorder::new(dir.path().join("harness"));

    let (one, two) = tokio::join!(whole_run(&recorder, "one"), whole_run(&recorder, "two"));
    assert_ne!(one, two);
    let path = recorder.path().to_path_buf();
    drop(recorder);

    let log = read_back(&path).await;
    for (run, owner) in [(one, "one"), (two, "two")] {
        let records = log.records(run, ALL).await.unwrap();
        let owners: Vec<&str> = records
            .iter()
            .map(|stored| stored.record.payload["owner"].as_str().unwrap())
            .collect();
        assert_eq!(owners, [owner; 3]);
        let task_seqs: Vec<u32> = records
            .iter()
            .map(|stored| stored.record.task_seq)
            .collect();
        assert_eq!(task_seqs, [0, 1, 2]);
    }
}
