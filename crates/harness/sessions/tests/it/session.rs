//! The session runtime end to end on an in-process harness: a launch
//! drives the program on the effect loop and records it in the run log; a
//! reconnecting client's transcript read matches what the log holds and
//! what a live subscriber saw; an operator's answer resumes the parked
//! program and the run completes; a turn-cancel relaunches the program as
//! a second run whose transcript indices continue; and a catalog whose
//! models changed retires the run. The close path - draining outstanding
//! effects and reporting the interrupt as one `Interrupted` failure -
//! lives in the `close` child module.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use harness_log::RunOutcome;
use harness_sessions::environment::{CatalogBinding, GatewayBinding};
use harness_sessions::input::{WaitError, WaitFrame};
use harness_sessions::protocol::{LaunchRequest, SessionEvent, SessionId};
use harness_sessions::runtime::{Harness, HarnessConfig, LaunchError};
use harness_sessions::session::Session;
use harness_sessions::transition::SessionState;
use tokio::sync::broadcast;

#[path = "session-close.rs"]
mod close;

/// A prompt that parks on operator input and returns it.
const ASKS: &str = "---\nname: asks\ndescription: asks the operator\npromptforge: 0\n---\n\n\
    # Asks\n\n## Only\n\n```lua\nreturn user_input()\n```\n";

/// How long a test waits for the supervisor to act.
const PATIENCE: Duration = Duration::from_secs(10);

/// A chat-capable catalog entry with no `id`: usable, so a session
/// launches under it, yet binding no model, so the gateway is never
/// contacted.
fn idless_chat_model() -> serde_json::Value {
    serde_json::json!({ "kind": "chat" })
}

/// A harness over a fresh agents directory holding `asks.md`, with a
/// usable (never contacted) gateway and no catalog bound yet.
fn unbound_harness(dir: &Path) -> Harness {
    let agents = dir.join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join("asks.md"), ASKS).unwrap();
    let harness = Harness::new(HarnessConfig {
        agents_path: agents,
        state_dir: dir.join("state"),
    });
    harness.set_gateway(GatewayBinding {
        base_url: "http://127.0.0.1:9".to_owned(),
        key: "k".to_owned(),
        generation: 1,
    });
    harness
}

/// [`unbound_harness`] with a usable catalog bound at generation 1.
fn harness(dir: &Path) -> Harness {
    let harness = unbound_harness(dir);
    harness.set_catalog(CatalogBinding {
        generation: 1,
        models: vec![idless_chat_model()],
    });
    harness
}

async fn launch(harness: &Harness) -> Session {
    harness
        .launch(LaunchRequest {
            agent: "asks".to_owned(),
            args: String::new(),
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
async fn an_unknown_agent_and_an_unbound_gateway_are_refused_at_launch() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let error = harness
        .launch(LaunchRequest {
            agent: "../etc/passwd".to_owned(),
            args: String::new(),
        })
        .await
        .expect_err("a path-shaped name is not a discovered agent");
    assert!(matches!(error, LaunchError::UnknownAgent { .. }), "{error}");

    let unbound = Harness::new(harness.config().clone());
    let error = unbound
        .launch(LaunchRequest {
            agent: "asks".to_owned(),
            args: String::new(),
        })
        .await
        .expect_err("no gateway means no model round could ever complete");
    assert!(matches!(error, LaunchError::GatewayUnusable), "{error}");
}

#[tokio::test]
async fn a_transcript_read_after_reconnect_matches_the_log_and_the_live_stream() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
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
    let transcript = reattached.transcript(0).await.unwrap();
    assert_eq!(transcript, seen, "the replay matches the live stream");

    // And it must be what the log holds, record for record.
    let log = harness.log().await.unwrap();
    let runs = reattached.run_ids();
    assert_eq!(runs.len(), 1, "one run so far");
    let records = log.lock().await.transcript(runs[0]).await.unwrap();
    assert_eq!(
        records
            .into_iter()
            .map(|stored| stored.record.payload)
            .collect::<Vec<_>>(),
        transcript
            .iter()
            .map(|event| event.event.clone())
            .collect::<Vec<_>>(),
        "the transcript is the log's event records in order"
    );

    // Resuming past a cursor skips what the client already has.
    let tail = reattached.transcript(2).await.unwrap();
    assert_eq!(tail, seen[2..].to_vec());

    assert!(harness.close(session.id()));
    wait_for(&session, SessionState::Closed).await;
}

#[tokio::test]
async fn an_answer_resumes_the_parked_wait_and_the_run_completes_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let session = launch(&harness).await;
    let mut waits = session.subscribe_waits();
    let token = required_token(&mut waits).await;

    // A refused answer (the accept-then-settle path) leaves the wait
    // open for the real one.
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
    let log = harness.log().await.unwrap();
    let runs = session.run_ids();
    assert_eq!(runs.len(), 1, "no relaunch: the program returned");
    let row = log.lock().await.run(runs[0]).await.unwrap();
    assert_eq!(
        row.outcome,
        Some(RunOutcome::Completed {
            final_text: "forty-two".to_owned()
        }),
        "the answer is the program's return value"
    );
    assert!(
        matches!(&session.transcript(0).await, Ok(events) if events.iter().any(|event| {
            event.event.get("kind").and_then(serde_json::Value::as_str) == Some("user_input")
                && event.event.get("text").and_then(serde_json::Value::as_str) == Some("forty-two")
        })),
        "the answer is recorded in the transcript"
    );
}

#[tokio::test]
async fn a_turn_cancel_relaunches_as_a_second_run_with_indices_continuing() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
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
    assert_eq!(runs.len(), 2, "the relaunch is a second run in the log");
    let log = harness.log().await.unwrap();
    let first = log.lock().await.run(runs[0]).await.unwrap();
    assert_eq!(first.outcome, Some(RunOutcome::Cancelled));
    let second = log.lock().await.run(runs[1]).await.unwrap();
    assert_eq!(second.outcome, None, "the second run is still parked");

    // Live indices continue across the relaunch, and the transcript read
    // from both runs' records agrees with the live stream index for index.
    let second_run_events = drain_live(&mut live, first_run_events);
    assert!(
        !second_run_events.is_empty(),
        "the second run reported events past the first run's"
    );
    seen.extend(second_run_events);
    let transcript = session.transcript(0).await.unwrap();
    assert_eq!(transcript, seen, "the replay spans both runs in order");
    let tail = session.transcript(first_run_events).await.unwrap();
    assert_eq!(
        tail.first().map(|event| event.index),
        Some(first_run_events),
        "the second run's first event takes the next index"
    );

    assert!(harness.close(session.id()));
    wait_for(&session, SessionState::Closed).await;
}

#[tokio::test]
async fn a_catalog_with_different_models_retires_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path());
    let session = launch(&harness).await;
    let mut waits = session.subscribe_waits();
    let first_token = required_token(&mut waits).await;

    // Same models, new generation: retained, the run keeps going.
    harness.set_catalog(CatalogBinding {
        generation: 2,
        models: vec![idless_chat_model()],
    });
    tokio::task::yield_now().await;
    assert_eq!(session.unresolved_waits(), vec![first_token.clone()]);

    // Different models: the frozen bindings are stale, so the run is
    // retired and the program relaunched under the new catalog. The entry
    // still carries no `id`, so the relaunch binds no model and never
    // contacts the gateway.
    harness.set_catalog(CatalogBinding {
        generation: 3,
        models: vec![serde_json::json!({ "kind": "chat", "description": "other" })],
    });
    cancelled_frame(&mut waits, &first_token).await;
    let second_token = required_token(&mut waits).await;
    assert_eq!(session.state(), SessionState::Alive);
    assert_eq!(session.unresolved_waits(), vec![second_token]);

    let runs = session.run_ids();
    assert_eq!(runs.len(), 2, "the retirement relaunched the program");
    let log = harness.log().await.unwrap();
    let first = log.lock().await.run(runs[0]).await.unwrap();
    assert_eq!(
        first.outcome,
        Some(RunOutcome::Cancelled),
        "the retired run closed its row as cancelled"
    );

    assert!(harness.close(session.id()));
    wait_for(&session, SessionState::Closed).await;
}

#[tokio::test]
async fn an_empty_catalog_holds_the_session_until_a_chat_model_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let harness = unbound_harness(dir.path());
    // A catalog with no chat-capable entry is pushed as an empty list: the
    // launch is acknowledged, but no run starts under it.
    harness.set_catalog(CatalogBinding {
        generation: 1,
        models: Vec::new(),
    });
    let session = launch(&harness).await;
    let mut waits = session.subscribe_waits();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), waits.recv())
            .await
            .is_err(),
        "no run starts while the catalog holds no chat-capable model"
    );
    assert!(session.run_ids().is_empty(), "no run row was opened");

    // The first usable generation starts the program.
    harness.set_catalog(CatalogBinding {
        generation: 2,
        models: vec![idless_chat_model()],
    });
    let _token = required_token(&mut waits).await;
    assert_eq!(
        session.run_ids().len(),
        1,
        "the usable catalog launched one run"
    );

    assert!(harness.close(session.id()));
    wait_for(&session, SessionState::Closed).await;
}
