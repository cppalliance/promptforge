//! The read path: the whole transcript for session views.

use serde_json::json;
use workshop_run_log::{LogError, Record, RecordKind, RunId, RunLog, RunMeta};

/// A run's opening row.
fn meta() -> RunMeta {
    RunMeta {
        session_id: "session-1".to_owned(),
        agent: "chat".to_owned(),
        prompt_hash: "sha256:abc".to_owned(),
        seed: 3,
        flags: 0,
        started_at: 1_700_000_000_000,
    }
}

/// One record with the given provenance and kind and an empty payload.
fn record(task_id: &str, task_seq: u32, kind: RecordKind) -> Record {
    Record {
        task_id: task_id.to_owned(),
        task_seq,
        kind,
        effect_id: (kind != RecordKind::Event).then_some(u64::from(task_seq)),
        payload: json!({}),
    }
}

/// Two tasks interleaved in loop order (the main walk `0` and its first
/// child `0.0`), each task's events arriving out of `task_seq` order, with
/// an effect and its answer mixed in so the event-only transcript has
/// something to exclude. Returns the run.
async fn interleaved_run(log: &mut RunLog) -> Result<RunId, LogError> {
    let run = log.begin_run(meta()).await?;
    let appended = [
        ("0", 0, RecordKind::Event),
        ("0.0", 2, RecordKind::Event),
        ("0", 1, RecordKind::Effect),
        ("0.0", 0, RecordKind::Event),
        ("0", 2, RecordKind::Answer),
        ("0", 3, RecordKind::Event),
        ("0.0", 1, RecordKind::Event),
        ("0", 4, RecordKind::Event),
    ];
    for (task_id, task_seq, kind) in appended {
        log.append(run, record(task_id, task_seq, kind)).await?;
    }
    Ok(run)
}

#[tokio::test]
async fn transcript_returns_every_event_in_seq_order_and_nothing_else() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = interleaved_run(&mut log).await.unwrap();

    let transcript = log.transcript(run).await.unwrap();
    let seqs: Vec<u64> = transcript.iter().map(|stored| stored.seq.get()).collect();
    assert_eq!(seqs, [0, 1, 3, 5, 6, 7]);
    let provenance: Vec<(&str, u32)> = transcript
        .iter()
        .map(|stored| (stored.record.task_id.as_str(), stored.record.task_seq))
        .collect();
    assert_eq!(
        provenance,
        [
            ("0", 0),
            ("0.0", 2),
            ("0.0", 0),
            ("0", 3),
            ("0.0", 1),
            ("0", 4)
        ]
    );
    assert!(
        transcript
            .iter()
            .all(|stored| stored.record.kind == RecordKind::Event)
    );
}

#[tokio::test]
async fn transcript_of_a_run_with_no_events_is_empty() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = log.begin_run(meta()).await.unwrap();
    log.append(run, record("0", 0, RecordKind::Effect))
        .await
        .unwrap();
    assert!(log.transcript(run).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_event_readers_refuse_an_unknown_run() {
    let log = RunLog::in_memory().await.unwrap();
    let ghost = RunId::from_raw(41);

    let transcript = log.transcript(ghost).await;
    assert!(matches!(transcript, Err(LogError::UnknownRun(id)) if id == ghost));
}
