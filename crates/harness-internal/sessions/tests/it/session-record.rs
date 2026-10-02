//! The Host's recorder at the session seam: the events of a prompt that
//! fails to prepare still reach the transcript, a recorder that refuses a
//! write ends the session's run as `RunFailed`, and a launch opens no file
//! of its own.

use std::sync::Arc;

use harness_runner::recorder::{
    MemoryRecorder, Record, RecordKind, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome,
    RunRecorder,
};
use harness_sessions::session::{FailureKind, Session, SessionFailure};
use harness_sessions::transition::SessionState;
use tokio::sync::broadcast;

use super::{
    PATIENCE, drain_live, harness, harness_over, launch, launch_agent, recorded_harness,
    required_token, wait_for,
};

/// A prompt with unclosed frontmatter, so it does not parse.
const BROKEN: &str = "---\nname: broken\ndescription: d\npromptforge: 0\n\n# Title\n";

/// Which write a [`RefusingRecorder`] refuses.
#[derive(Clone, Copy)]
enum Refuses {
    /// The start of a run.
    Begin,
    /// The first effect record of a run, which the effect loop appends.
    FirstEffect,
}

/// A recorder over a [`MemoryRecorder`] that refuses one kind of write
/// and keeps everything else.
struct RefusingRecorder {
    inner: MemoryRecorder,
    refuses: Refuses,
}

impl RefusingRecorder {
    fn new(refuses: Refuses) -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryRecorder::new(),
            refuses,
        })
    }
}

impl RunRecorder for RefusingRecorder {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        Box::pin(async move {
            if matches!(self.refuses, Refuses::Begin) {
                return Err(RecorderError::new("the fixture recorder refuses to begin"));
            }
            self.inner.begin_run(meta).await
        })
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            if matches!(self.refuses, Refuses::FirstEffect) && record.kind == RecordKind::Effect {
                return Err(RecorderError::new("the fixture recorder refuses an effect"));
            }
            self.inner.append(run, record).await
        })
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        self.inner.end_run(run, outcome)
    }
}

/// Waits for the session to end and returns the one failure it reported.
async fn the_reported_failure(
    session: &Session,
    errors: &mut broadcast::Receiver<SessionFailure>,
) -> SessionFailure {
    wait_for(session, SessionState::Closed).await;
    tokio::time::timeout(PATIENCE, errors.recv())
        .await
        .expect("the failure is reported in time")
        .expect("the failure report arrives")
}

#[tokio::test]
async fn the_events_of_a_prompt_that_does_not_parse_reach_the_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let (harness, recorder) = recorded_harness(dir.path());
    std::fs::write(dir.path().join("agents").join("broken.md"), BROKEN).unwrap();
    let session = launch_agent(&harness, "broken").await;
    let mut live = session.subscribe_events();
    let mut errors = session.subscribe_errors();

    let failure = the_reported_failure(&session, &mut errors).await;
    assert_eq!(failure.kind, FailureKind::RunFailed);

    let runs = session.run_ids();
    assert_eq!(runs.len(), 1, "the failed preparation began one run");
    assert!(
        matches!(recorder.outcome(runs[0]), Some(RunOutcome::Failed { .. })),
        "the recorder holds the run as failed"
    );
    let stored: Vec<_> = recorder
        .records(runs[0])
        .into_iter()
        .map(|record| record.payload)
        .collect();
    let transcript = session.transcript(0);
    assert_eq!(
        transcript
            .iter()
            .map(|entry| entry.event.clone())
            .collect::<Vec<_>>(),
        stored,
        "the transcript holds the events the failed preparation recorded"
    );
    assert_eq!(
        transcript.first().map(|entry| entry.event["kind"].clone()),
        Some(serde_json::json!("parse_started")),
        "the transcript opens with the parse"
    );
    assert_eq!(
        drain_live(&mut live, 0),
        transcript,
        "the live stream saw the same entries"
    );
}

#[tokio::test]
async fn a_recorder_that_refuses_to_begin_a_run_ends_the_session_as_run_failed() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = RefusingRecorder::new(Refuses::Begin);
    let harness = harness_over(dir.path(), recorder);
    let session = launch(&harness).await;
    let mut errors = session.subscribe_errors();

    let failure = the_reported_failure(&session, &mut errors).await;
    assert_eq!(failure.kind, FailureKind::RunFailed);
    assert!(
        failure.message.contains("the run recorder failed")
            && failure.message.contains("refuses to begin"),
        "the report names the recorder and its cause: {}",
        failure.message
    );
    assert!(session.run_ids().is_empty(), "no run began");
    assert!(session.transcript(0).is_empty(), "nothing was observed");
}

#[tokio::test]
async fn a_recorder_that_refuses_an_effect_ends_the_session_as_run_failed() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = RefusingRecorder::new(Refuses::FirstEffect);
    let shared = Arc::clone(&recorder);
    let harness = harness_over(dir.path(), shared);
    let session = launch(&harness).await;
    let mut errors = session.subscribe_errors();

    let failure = the_reported_failure(&session, &mut errors).await;
    assert_eq!(failure.kind, FailureKind::RunFailed);
    assert!(
        failure.message.contains("the run recorder failed")
            && failure.message.contains("refuses an effect"),
        "the report names the recorder and its cause: {}",
        failure.message
    );
    let runs = session.run_ids();
    assert_eq!(runs.len(), 1, "the run began before the refusal");
    assert_eq!(
        recorder.inner.outcome(runs[0]),
        None,
        "a refused write leaves the run open"
    );
    assert!(
        recorder
            .inner
            .records(runs[0])
            .iter()
            .all(|record| record.kind == RecordKind::Event),
        "the loop stopped at the refused effect"
    );
    assert!(
        !session.transcript(0).is_empty(),
        "the events recorded before the refusal reached the transcript"
    );
}

#[tokio::test]
async fn a_launch_creates_no_file_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let session = launch(&harness).await;
    let mut waits = session.subscribe_waits();
    let _token = required_token(&mut waits).await;

    let mut names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["agents".to_owned()],
        "the launch and its running session wrote nothing beside the agents directory"
    );

    assert!(harness.close(session.id()));
    wait_for(&session, SessionState::Closed).await;
}
