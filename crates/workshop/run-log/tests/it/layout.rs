//! The layout stamp: a file written in another layout is set aside, never
//! written into, and a file in the current layout reopens in place.

// clippy.toml's allow-unwrap-in-tests covers #[test] functions only, not
// the helpers they share; failing a test by panicking is what these are for.
#![expect(
    clippy::unwrap_used,
    reason = "test helpers fail by panicking with the failure shown"
)]

use std::path::{Path, PathBuf};

use workshop_run_log::{RunLog, RunMeta, RunOutcome};

/// The tables as the log wrote them before runs were named: a
/// `session_id` column where `name` is now, and no `layout` table.
const SESSION_LAYOUT: &str = "
CREATE TABLE runs (
    run_id        INTEGER PRIMARY KEY,
    session_id    TEXT    NOT NULL,
    agent         TEXT    NOT NULL,
    prompt_hash   TEXT    NOT NULL,
    seed          INTEGER NOT NULL,
    flags         INTEGER NOT NULL,
    started_at    INTEGER NOT NULL,
    ended_at      INTEGER,
    outcome       TEXT,
    final_text    TEXT,
    error_kind    TEXT,
    error_message TEXT
);
CREATE TABLE records (
    run_id    INTEGER NOT NULL,
    seq       INTEGER NOT NULL,
    task_id   TEXT    NOT NULL,
    task_seq  INTEGER NOT NULL,
    kind      TEXT    NOT NULL,
    effect_id INTEGER,
    payload   TEXT    NOT NULL,
    at        INTEGER NOT NULL,
    PRIMARY KEY (run_id, seq)
);
INSERT INTO runs (session_id, agent, prompt_hash, seed, flags, started_at)
    VALUES ('session-old', 'chat', 'sha256:old', 0, 0, 1);
";

/// A run's opening row as the Harness would write it.
fn meta() -> RunMeta {
    RunMeta {
        name: "conversation-1".to_owned(),
        prompt_hash: "sha256:abc".to_owned(),
        seed: 7,
        flags: 0,
        started_at: 1_700_000_000_000,
    }
}

/// A raw connection to `path`, past the log.
async fn raw(path: &Path) -> turso::Connection {
    let database = turso::Builder::new_local(path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    database.connect().unwrap()
}

/// The set-aside databases in `dir`: every file named `runs.db.stale-*`
/// that is not a sidecar.
fn set_aside(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            let name = path.file_name().unwrap().to_str().unwrap();
            name.starts_with("runs.db.stale-") && !name.ends_with("-wal") && !name.ends_with("-shm")
        })
        .collect()
}

/// The layout versions stamped into the file at `path`, in row order.
async fn stamps(path: &Path) -> Vec<i64> {
    let mut rows = raw(path)
        .await
        .query("SELECT version FROM layout", ())
        .await
        .unwrap();
    let mut versions = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        versions.push(row.get::<i64>(0).unwrap());
    }
    versions
}

/// A run begins, takes nothing, and ends in `log`.
async fn round_trip(log: &mut RunLog) {
    let run = log.begin_run(meta(), "chat").await.unwrap();
    log.end_run(run, RunOutcome::Cancelled).await.unwrap();
    let row = log.run(run).await.unwrap();
    assert_eq!(row.meta, meta());
    assert_eq!(row.agent, "chat");
}

#[tokio::test]
async fn a_file_from_before_runs_were_named_is_set_aside_with_its_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.db");
    raw(&path)
        .await
        .execute_batch(SESSION_LAYOUT)
        .await
        .unwrap();

    let mut log = RunLog::open(&path).await.unwrap();
    round_trip(&mut log).await;
    drop(log);

    let aside = set_aside(dir.path());
    assert_eq!(aside.len(), 1, "the old file is kept beside the new one");
    let mut rows = raw(&aside[0])
        .await
        .query("SELECT session_id FROM runs", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), "session-old");
}

#[tokio::test]
async fn a_file_stamped_with_another_layout_is_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.db");
    drop(RunLog::open(&path).await.unwrap());
    raw(&path)
        .await
        .execute("UPDATE layout SET version = version + 1", ())
        .await
        .unwrap();

    let mut log = RunLog::open(&path).await.unwrap();
    round_trip(&mut log).await;
    drop(log);

    assert_eq!(set_aside(dir.path()).len(), 1);
}

#[tokio::test]
async fn an_unstamped_file_with_no_runs_is_stamped_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.db");
    drop(RunLog::open(&path).await.unwrap());
    let current = stamps(&path).await;
    // A second open racing the first sees the tables before their stamp.
    raw(&path)
        .await
        .execute("DELETE FROM layout", ())
        .await
        .unwrap();

    let mut log = RunLog::open(&path).await.unwrap();
    round_trip(&mut log).await;
    drop(log);

    assert!(
        set_aside(dir.path()).is_empty(),
        "a file with no runs has nothing to set aside"
    );
    assert_eq!(stamps(&path).await, current, "the reopen stamps the file");
}

#[tokio::test]
async fn an_unstamped_file_with_runs_is_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.db");
    let mut log = RunLog::open(&path).await.unwrap();
    round_trip(&mut log).await;
    drop(log);
    raw(&path)
        .await
        .execute("DELETE FROM layout", ())
        .await
        .unwrap();

    let mut log = RunLog::open(&path).await.unwrap();
    round_trip(&mut log).await;
    drop(log);

    assert_eq!(
        set_aside(dir.path()).len(),
        1,
        "a file whose runs carry no stamp is kept beside the new one"
    );
}

#[tokio::test]
async fn a_file_in_the_current_layout_reopens_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.db");
    let run = {
        let mut log = RunLog::open(&path).await.unwrap();
        let run = log.begin_run(meta(), "chat").await.unwrap();
        log.end_run(run, RunOutcome::Cancelled).await.unwrap();
        run
    };

    let log = RunLog::open(&path).await.unwrap();
    assert_eq!(log.run(run).await.unwrap().meta, meta());
    assert!(
        set_aside(dir.path()).is_empty(),
        "a current file is never set aside"
    );
}
