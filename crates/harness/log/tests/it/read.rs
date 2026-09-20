//! The read path: per-task event slices for `TaskEvents` and the whole
//! transcript for session views.

use harness_log::{LogError, Record, RecordKind, RunId, RunLog, RunMeta};
use serde_json::json;

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

/// One record whose payload names its own provenance and kind, so a
/// returned payload proves which row it came from.
fn record(task_id: &str, task_seq: u32, kind: RecordKind) -> Record {
    Record {
        task_id: task_id.to_owned(),
        task_seq,
        kind,
        effect_id: (kind != RecordKind::Event).then_some(u64::from(task_seq)),
        payload: json!({ "task": task_id, "task_seq": task_seq, "kind": kind.as_str() }),
    }
}

/// Two tasks interleaved in loop order (the main walk `0` and its first
/// child `0.0`), each task's events arriving out of `task_seq` order, with
/// an effect and its answer mixed in so the event-only readers have
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

/// The `task_seq` each payload claims; a payload without one reads as
/// `None`, which no assertion below accepts.
fn task_seqs(payloads: &[serde_json::Value]) -> Vec<Option<u64>> {
    payloads
        .iter()
        .map(|payload| payload["task_seq"].as_u64())
        .collect()
}

/// The `task_seq`s an assertion expects, in `task_seqs`'s shape.
fn expected<const N: usize>(seqs: [u64; N]) -> Vec<Option<u64>> {
    seqs.into_iter().map(Some).collect()
}

#[tokio::test]
async fn events_for_task_returns_one_task_in_task_seq_order_when_tasks_interleave() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = interleaved_run(&mut log).await.unwrap();

    let task_zero = log.events_for_task(run, "0", None).await.unwrap();
    assert_eq!(task_seqs(&task_zero), expected([0, 3, 4]));
    for payload in &task_zero {
        assert_eq!(payload["task"], json!("0"));
        assert_eq!(payload["kind"], json!("event"));
    }

    let task_one = log.events_for_task(run, "0.0", None).await.unwrap();
    assert_eq!(task_seqs(&task_one), expected([0, 1, 2]));
    for payload in &task_one {
        assert_eq!(payload["task"], json!("0.0"));
    }
}

#[tokio::test]
async fn events_for_task_last_n_keeps_the_final_n_by_task_seq() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = interleaved_run(&mut log).await.unwrap();

    let last_two = log.events_for_task(run, "0.0", Some(2)).await.unwrap();
    assert_eq!(task_seqs(&last_two), expected([1, 2]));

    let last_one = log.events_for_task(run, "0", Some(1)).await.unwrap();
    assert_eq!(task_seqs(&last_one), expected([4]));

    let more_than_exist = log.events_for_task(run, "0.0", Some(10)).await.unwrap();
    assert_eq!(task_seqs(&more_than_exist), expected([0, 1, 2]));

    let none = log.events_for_task(run, "0.0", Some(0)).await.unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn events_for_task_is_empty_for_a_task_that_never_logged() {
    let mut log = RunLog::in_memory().await.unwrap();
    let run = interleaved_run(&mut log).await.unwrap();
    assert!(
        log.events_for_task(run, "0.9", None)
            .await
            .unwrap()
            .is_empty()
    );
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

    let events = log.events_for_task(ghost, "0", None).await;
    assert!(matches!(events, Err(LogError::UnknownRun(id)) if id == ghost));

    let transcript = log.transcript(ghost).await;
    assert!(matches!(transcript, Err(LogError::UnknownRun(id)) if id == ghost));
}
