//! The session runtime end to end on an in-process harness: a launch
//! drives the program on the effect loop and records it through the
//! Host's recorder; a reconnecting client's transcript read matches what
//! the recorder holds and what a live subscriber saw; an operator's answer
//! resumes the parked program and the run completes; and a turn-cancel
//! relaunches the program as a second run whose transcript indices
//! continue. The close path - draining outstanding effects and reporting
//! the interrupt as one
//! `Interrupted` failure - sits in the `close` child module, the
//! capabilities runs resolve against in the `capabilities` child module,
//! the prompt's declared input and output files in the `files` child
//! module, the model a launch binds through the broker in the `model`
//! child module, and the recorder's own failures and the transcript of a
//! failed run in the `record` child module.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use harness_capabilities::{CapabilityRegistry, HostServices, USER_INPUT_ASK_TOOL, UserInput};
use harness_runner::recorder::{MemoryRecorder, RecordKind, RunOutcome, RunRecorder};
use harness_sessions::input::{WaitError, WaitFrame};
use harness_sessions::protocol::{LaunchRequest, SessionEvent, SessionId};
use harness_sessions::runtime::{Harness, HarnessConfig, LaunchError};
use harness_sessions::session::Session;
use harness_sessions::transition::SessionState;
use tokio::sync::broadcast;

use crate::support::OfflineBroker;

#[path = "session-capabilities.rs"]
mod capabilities;

#[path = "session-close.rs"]
mod close;

#[path = "session-files.rs"]
mod files;

#[path = "session-infer.rs"]
mod infer;

#[path = "session-model.rs"]
mod model;

#[path = "session-record.rs"]
mod record;

/// A prompt that parks on operator input and returns it.
const ASKS: &str = "---\nname: asks\ndescription: asks the operator\npromptforge: 0\n\
    capabilities:\n  - promptforge/user-input\n---\n\n\
    # Asks\n\n## Only\n\n```lua\nreturn (input.ask())\n```\n";

/// How long a test waits for the supervisor to act.
const PATIENCE: Duration = Duration::from_secs(10);

/// A registry holding `promptforge/user-input`, which `asks.md` declares.
fn user_input_registry() -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    registry.register(Arc::new(UserInput::new())).unwrap();
    registry
}

/// A Harness over a fresh agents directory holding `asks.md`, recording
/// through `recorder` and resolving capabilities against `capabilities`,
/// on the offline broker, which leaves every run without a model.
fn harness_with_capabilities(
    dir: &Path,
    recorder: Arc<dyn RunRecorder>,
    capabilities: CapabilityRegistry,
) -> Harness {
    let agents = dir.join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("asks.md"), ASKS).unwrap();
    Harness::new(
        HarnessConfig {
            agents_path: agents,
        },
        recorder,
        Arc::new(OfflineBroker),
        capabilities,
        HostServices::new(),
    )
}

/// [`harness_with_capabilities`] user input.
fn harness_over(dir: &Path, recorder: Arc<dyn RunRecorder>) -> Harness {
    harness_with_capabilities(dir, recorder, user_input_registry())
}

/// [`harness_over`] a fresh [`MemoryRecorder`], which the Harness alone
/// holds.
fn harness(dir: &Path) -> Harness {
    harness_over(dir, Arc::new(MemoryRecorder::new()))
}

/// A Harness recording to a [`MemoryRecorder`] the test keeps, so it can
/// read back what the sessions recorded.
fn recorded_harness(dir: &Path) -> (Harness, Arc<MemoryRecorder>) {
    let recorder = Arc::new(MemoryRecorder::new());
    let shared = Arc::clone(&recorder);
    (harness_over(dir, shared), recorder)
}

async fn launch(harness: &Harness) -> Session {
    harness
        .launch(LaunchRequest {
            agent: "asks".to_owned(),
            args: String::new(),
            input_text: None,
        })
        .await
        .expect("the discovered agent launches")
}

/// Launches the discovered agent named `agent`.
async fn launch_agent(harness: &Harness, agent: &str) -> Session {
    harness
        .launch(LaunchRequest {
            agent: agent.to_owned(),
            args: String::new(),
            input_text: None,
        })
        .await
        .expect("the discovered agent launches")
}

/// Waits for the run to park on its input wait.
async fn required_token(waits: &mut broadcast::Receiver<WaitFrame>) -> String {
    let frame = tokio::time::timeout(PATIENCE, waits.recv())
        .await
        .expect("the run reaches its input wait in time")
        .expect("the wait frame arrives");
    let WaitFrame::Required { token } = frame else {
        panic!("expected a required frame first, got {frame:?}");
    };
    token
}

/// Waits for the wait holding `token` to die as cancelled.
async fn cancelled_frame(waits: &mut broadcast::Receiver<WaitFrame>, token: &str) {
    let frame = tokio::time::timeout(PATIENCE, waits.recv())
        .await
        .expect("the retired run's wait dies in time")
        .expect("the cancelled frame arrives");
    assert_eq!(
        frame,
        WaitFrame::Cancelled {
            token: token.to_owned()
        }
    );
}

/// Drains the live event stream and checks it is numbered from `from`
/// without gaps, returning what was seen.
fn drain_live(live: &mut broadcast::Receiver<SessionEvent>, from: u64) -> Vec<SessionEvent> {
    let mut seen: Vec<SessionEvent> = Vec::new();
    while let Ok(event) = live.try_recv() {
        seen.push(event);
    }
    assert_eq!(
        seen.iter().map(|event| event.index).collect::<Vec<_>>(),
        (from..from + seen.len() as u64).collect::<Vec<_>>(),
        "live events are numbered from {from} without gaps"
    );
    seen
}

/// Waits until the session reports `state`.
async fn wait_for(session: &Session, state: SessionState) {
    let mut watch = session.subscribe_state();
    tokio::time::timeout(PATIENCE, watch.wait_for(|current| *current == state))
        .await
        .expect("the session reaches the state in time")
        .expect("the session's state watch stays open");
}

#[tokio::test]
async fn an_unknown_agent_is_refused_at_launch() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let error = harness
        .launch(LaunchRequest {
            agent: "../etc/passwd".to_owned(),
            args: String::new(),
            input_text: None,
        })
        .await
        .expect_err("a path-shaped name is not a discovered agent");
    assert!(matches!(error, LaunchError::UnknownAgent { .. }), "{error}");
}

#[tokio::test]
async fn a_transcript_read_after_reconnect_matches_the_recorder_and_the_live_stream() {
    let dir = tempfile::tempdir().unwrap();
    let (harness, recorder) = recorded_harness(dir.path());
    let session = launch(&harness).await;
    let mut live = session.subscribe_events();
    let mut waits = session.subscribe_waits();
    let _token = required_token(&mut waits).await;

    // Everything the run reported before parking has been broadcast.
    let seen = drain_live(&mut live, 0);
    assert!(!seen.is_empty(), "the run reported events before parking");

    // A reconnecting client looks the session up by id and reads the
    // transcript: it must be what the live subscriber saw.
    let reattached = harness
        .session(&SessionId::new(session.id().as_str()))
        .expect("the session outlives the first handle");
    let transcript = reattached.transcript(0);
    assert_eq!(transcript, seen, "the replay matches the live stream");

    // And it must be what the recorder holds, record for record.
    let runs = reattached.run_ids();
    assert_eq!(runs.len(), 1, "one run so far");
    let stored: Vec<_> = recorder
        .records(runs[0])
        .into_iter()
        .filter(|record| record.kind == RecordKind::Event)
        .map(|record| record.payload)
        .collect();
    assert_eq!(
        stored,
        transcript
            .iter()
            .map(|event| event.event.clone())
            .collect::<Vec<_>>(),
        "the transcript is the recorder's event records in order"
    );

    // Resuming past a cursor skips what the client already has.
    let tail = reattached.transcript(2);
    assert_eq!(tail, seen[2..].to_vec());
    assert!(
        reattached.transcript(seen.len() as u64).is_empty(),
        "a cursor past the last event reads nothing"
    );

    assert!(harness.close(session.id()));
    wait_for(&session, SessionState::Closed).await;
}

#[tokio::test]
async fn an_answer_resumes_the_parked_wait_and_the_run_completes_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let (harness, recorder) = recorded_harness(dir.path());
    let session = launch(&harness).await;
    let mut waits = session.subscribe_waits();
    let token = required_token(&mut waits).await;

    // A refused answer leaves the wait open for the real one.
    let refused = session
        .send_input("not-a-token", "ignored".to_owned(), || {})
        .expect_err("an unknown token is refused");
    assert!(matches!(refused, WaitError::UnknownToken), "{refused}");
    assert_eq!(session.unresolved_waits(), vec![token.clone()]);

    let resumed = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&resumed);
    session
        .send_input(&token, "forty-two".to_owned(), move || {
            flag.store(true, Ordering::SeqCst);
        })
        .expect("the open wait takes the answer");
    assert!(
        resumed.load(Ordering::SeqCst),
        "the client's turn bookkeeping runs once the answer is accepted"
    );
    assert!(
        session.unresolved_waits().is_empty(),
        "the token is consumed"
    );

    // The program returned the answer, so the run completed and the
    // session ended on its own.
    wait_for(&session, SessionState::Closed).await;
    assert!(
        harness.session(session.id()).is_none(),
        "a finished session leaves the harness"
    );
    let runs = session.run_ids();
    assert_eq!(runs.len(), 1, "no relaunch: the program returned");
    assert_eq!(
        recorder.outcome(runs[0]),
        Some(RunOutcome::Completed {
            final_text: "forty-two".to_owned()
        }),
        "the answer is the program's return value"
    );
    assert!(
        session.transcript(0).iter().any(|event| {
            let field = |name: &str| event.event.get(name);
            field("kind").and_then(serde_json::Value::as_str) == Some("tool_result")
                && field("alias").and_then(serde_json::Value::as_str) == Some(USER_INPUT_ASK_TOOL)
                && field("tool_call_id").and_then(serde_json::Value::as_str) == Some("")
                && field("content").and_then(serde_json::Value::as_str) == Some("forty-two")
                && field("trusted").and_then(serde_json::Value::as_bool) == Some(true)
        }),
        "the answer is recorded in the transcript as the ask tool's trusted result"
    );
}

#[tokio::test]
async fn a_turn_cancel_relaunches_as_a_second_run_with_indices_continuing() {
    let dir = tempfile::tempdir().unwrap();
    let (harness, recorder) = recorded_harness(dir.path());
    let session = launch(&harness).await;
    let mut live = session.subscribe_events();
    let mut waits = session.subscribe_waits();
    let first_token = required_token(&mut waits).await;
    let mut seen = drain_live(&mut live, 0);
    let first_run_events = seen.len() as u64;
    assert!(first_run_events > 0);

    session.cancel();

    // The first run dies as a stop reason and the program is relaunched
    // over the retained transcript: it parks again under a fresh token.
    cancelled_frame(&mut waits, &first_token).await;
    let second_token = required_token(&mut waits).await;
    assert_ne!(second_token, first_token, "wait tokens are single-use");
    assert_eq!(
        session.state(),
        SessionState::Alive,
        "a turn-cancel does not end the session"
    );
    assert_eq!(session.unresolved_waits(), vec![second_token]);

    let runs = session.run_ids();
    assert_eq!(
        runs.len(),
        2,
        "the relaunch is a second run at the recorder"
    );
    assert_eq!(recorder.outcome(runs[0]), Some(RunOutcome::Cancelled));
    assert_eq!(
        recorder.outcome(runs[1]),
        None,
        "the second run is still parked"
    );

    // Live indices continue across the relaunch, and the transcript of
    // both runs agrees with the live stream index for index.
    let second_run_events = drain_live(&mut live, first_run_events);
    assert!(
        !second_run_events.is_empty(),
        "the second run reported events past the first run's"
    );
    seen.extend(second_run_events);
    let transcript = session.transcript(0);
    assert_eq!(transcript, seen, "the replay spans both runs in order");
    let tail = session.transcript(first_run_events);
    assert_eq!(
        tail.first().map(|event| event.index),
        Some(first_run_events),
        "the second run's first event takes the next index"
    );

    assert!(harness.close(session.id()));
    wait_for(&session, SessionState::Closed).await;
}
