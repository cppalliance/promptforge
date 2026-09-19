//! The write path: a run begins, records append in loop order, the run ends.

use harness_log::{LogError, Record, RecordFilter, RecordKind, RunId, RunLog, RunMeta, RunOutcome};
use serde_json::json;

/// Every record of a run, in loop order.
const ALL: RecordFilter = RecordFilter {
    kind: None,
    task: None,
    last: None,
};

/// A run's opening row as the harness would write it.
fn meta() -> RunMeta {
    RunMeta {
        session_id: "session-1".to_owned(),
        agent: "chat".to_owned(),
        prompt_hash: "sha256:abc".to_owned(),
        seed: u64::MAX - 1,
        flags: 0,
        started_at: 1_700_000_000_000,
    }
}

/// One record of `kind` on task 0 at `task_seq`.
fn record(kind: RecordKind, task_seq: u32, effect_id: Option<u64>) -> Record {
    Record {
        task_id: 0,
        task_seq,
        kind,
        effect_id,
        payload: json!({ "task_seq": task_seq }),
    }
}

#[tokio::test]
async fn a_run_with_three_records_round_trips_through_an_in_memory_log() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = log.begin_run(meta()).await.unwrap();

    log.append(run, record(RecordKind::Effect, 0, Some(7)))
        .await
        .unwrap();
    log.append(run, record(RecordKind::Answer, 1, Some(7)))
        .await
        .unwrap();
    log.append(run, record(RecordKind::Event, 2, None))
        .await
        .unwrap();

    let row = log.run(run).await.unwrap();
    assert_eq!(row.id, run);
    assert_eq!(row.meta, meta());
    assert_eq!(row.ended_at, None);
    assert_eq!(row.outcome, None);

    let records = log.records(run, ALL).await.unwrap();
    assert_eq!(records.len(), 3);
    let kinds: Vec<RecordKind> = records.iter().map(|stored| stored.record.kind).collect();
    assert_eq!(
        kinds,
        [RecordKind::Effect, RecordKind::Answer, RecordKind::Event]
    );
    let effect_ids: Vec<Option<u64>> = records
        .iter()
        .map(|stored| stored.record.effect_id)
        .collect();
    assert_eq!(effect_ids, [Some(7), Some(7), None]);
    for (index, stored) in records.iter().enumerate() {
        let task_seq = u32::try_from(index).unwrap();
        assert_eq!(stored.record.task_id, 0);
        assert_eq!(stored.record.task_seq, task_seq);
        assert_eq!(stored.record.payload, json!({ "task_seq": task_seq }));
        assert!(stored.at >= row.meta.started_at);
    }
}

#[tokio::test]
async fn seq_is_assigned_in_call_order_and_strictly_increases() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = log.begin_run(meta()).await.unwrap();

    let mut seqs = Vec::new();
    for task_seq in 0..5 {
        let seq = log
            .append(run, record(RecordKind::Event, task_seq, None))
            .await
            .unwrap();
        seqs.push(seq);
    }
    let raw: Vec<u64> = seqs.iter().map(|seq| seq.get()).collect();
    assert_eq!(raw, [0, 1, 2, 3, 4]);

    let stored: Vec<u64> = log
        .records(run, ALL)
        .await
        .unwrap()
        .iter()
        .map(|stored| stored.seq.get())
        .collect();
    assert_eq!(stored, raw);
}

#[tokio::test]
async fn seq_is_per_run_so_two_runs_each_start_at_zero() {
    let mut log = RunLog::in_memory().await.unwrap();
    let first = log.begin_run(meta()).await.unwrap();
    let second = log.begin_run(meta()).await.unwrap();
    assert_ne!(first, second);

    log.append(first, record(RecordKind::Event, 0, None))
        .await
        .unwrap();
    let seq = log
        .append(second, record(RecordKind::Event, 0, None))
        .await
        .unwrap();
    assert_eq!(seq.get(), 0);
    assert_eq!(log.records(first, ALL).await.unwrap().len(), 1);
    assert_eq!(log.records(second, ALL).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_filter_selects_by_kind_and_task_and_keeps_the_last_n() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = log.begin_run(meta()).await.unwrap();
    // Two tasks interleaved; task 1's records arrive out of `task_seq`
    // order so the per-task slice must sort by `task_seq`, not `seq`.
    let appended = [
        (0, 0, RecordKind::Event),
        (1, 1, RecordKind::Event),
        (0, 1, RecordKind::Effect),
        (1, 0, RecordKind::Event),
        (0, 2, RecordKind::Event),
    ];
    for (task_id, task_seq, kind) in appended {
        let mut record = record(kind, task_seq, None);
        record.task_id = task_id;
        log.append(run, record).await.unwrap();
    }
    let positions = |records: Vec<harness_log::StoredRecord>| -> Vec<(u64, u32)> {
        records
            .into_iter()
            .map(|stored| (stored.record.task_id, stored.record.task_seq))
            .collect()
    };

    let events = RecordFilter {
        kind: Some(RecordKind::Event),
        ..ALL
    };
    let all_events = log.records(run, events).await.unwrap();
    assert_eq!(positions(all_events), [(0, 0), (1, 1), (1, 0), (0, 2)]);

    let task_one = log
        .records(
            run,
            RecordFilter {
                task: Some(1),
                ..ALL
            },
        )
        .await
        .unwrap();
    assert_eq!(positions(task_one), [(1, 0), (1, 1)]);

    let last_two_of_task_zero = log
        .records(
            run,
            RecordFilter {
                task: Some(0),
                last: Some(2),
                ..ALL
            },
        )
        .await
        .unwrap();
    assert_eq!(positions(last_two_of_task_zero), [(0, 1), (0, 2)]);

    let last_event = log
        .records(
            run,
            RecordFilter {
                last: Some(1),
                ..events
            },
        )
        .await
        .unwrap();
    assert_eq!(positions(last_event), [(0, 2)]);
}

#[tokio::test]
async fn end_run_fills_ended_at_and_outcome() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = log.begin_run(meta()).await.unwrap();
    log.end_run(
        run,
        RunOutcome::Completed {
            final_text: "done".to_owned(),
        },
    )
    .await
    .unwrap();

    let row = log.run(run).await.unwrap();
    assert!(row.ended_at.is_some_and(|at| at >= row.meta.started_at));
    assert_eq!(
        row.outcome,
        Some(RunOutcome::Completed {
            final_text: "done".to_owned(),
        })
    );
}

#[tokio::test]
async fn a_failed_outcome_keeps_its_kind_and_message() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = log.begin_run(meta()).await.unwrap();
    let outcome = RunOutcome::Failed {
        kind: "tool".to_owned(),
        message: "expected a reply, got nothing".to_owned(),
    };
    log.end_run(run, outcome.clone()).await.unwrap();
    assert_eq!(log.run(run).await.unwrap().outcome, Some(outcome));

    let run = log.begin_run(meta()).await.unwrap();
    log.end_run(run, RunOutcome::Cancelled).await.unwrap();
    assert_eq!(
        log.run(run).await.unwrap().outcome,
        Some(RunOutcome::Cancelled)
    );
}

#[tokio::test]
async fn a_run_ends_exactly_once() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = log.begin_run(meta()).await.unwrap();
    log.end_run(run, RunOutcome::Cancelled).await.unwrap();

    let again = log.end_run(run, RunOutcome::Cancelled).await;
    assert!(matches!(again, Err(LogError::RunEnded(id)) if id == run));

    let late = log.append(run, record(RecordKind::Event, 0, None)).await;
    assert!(matches!(late, Err(LogError::RunEnded(id)) if id == run));
}

#[tokio::test]
async fn an_unknown_run_is_refused() {
    let mut log = RunLog::in_memory().await.unwrap();
    let ghost = RunId::from_raw(41);

    let appended = log.append(ghost, record(RecordKind::Event, 0, None)).await;
    assert!(matches!(appended, Err(LogError::UnknownRun(id)) if id == ghost));

    let ended = log.end_run(ghost, RunOutcome::Cancelled).await;
    assert!(matches!(ended, Err(LogError::UnknownRun(id)) if id == ghost));

    let read = log.run(ghost).await;
    assert!(matches!(read, Err(LogError::UnknownRun(id)) if id == ghost));
}

#[tokio::test]
async fn a_log_on_disk_keeps_its_rows_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.db");

    let run = {
        let mut log = RunLog::open(&path).await.unwrap();
        let run = log.begin_run(meta()).await.unwrap();
        log.append(run, record(RecordKind::Event, 0, None))
            .await
            .unwrap();
        log.end_run(run, RunOutcome::Cancelled).await.unwrap();
        run
    };

    let log = RunLog::open(&path).await.unwrap();
    let row = log.run(run).await.unwrap();
    assert_eq!(row.outcome, Some(RunOutcome::Cancelled));
    assert_eq!(log.records(run, ALL).await.unwrap().len(), 1);
}
