//! Tests for the input wait registry, its tokens, and the operator input broker.

use super::*;

use std::sync::Arc;

use harness_runner::performers::InputPerformer;
use harness_runner::spawn::spawn_tagged;
use harness_runner::test_support::mock_tag;
use promptforge_api_runtime::input::InputOutcome;
use promptforge_api_runtime::{Effect, EffectAnswer, Prompt, Run, RunContext, RunResult, Step};
use promptforge_api_types::timestamp::Timestamp;

/// Hostile operator text covering the bytes most likely to be mangled
/// by an envelope or codec.
const GNARLY: &str = "line1\r\nline2 \"quoted\" {\"text\":\"decoy\"} \\slash \u{1F980}";

async fn required_token(socket: &mut broadcast::Receiver<WaitFrame>) -> String {
    let frame = socket.recv().await.expect("a frame arrives");
    let WaitFrame::Required { token } = frame else {
        panic!("expected a required frame first, got {frame:?}");
    };
    token
}

#[test]
fn complete_delivers_the_value_and_consumes_the_token() {
    let registry = WaitRegistry::new();
    let (token, mut receiver) = registry.create();
    registry
        .complete(&token, "hello".to_owned())
        .expect("a live wait completes");
    assert_eq!(
        receiver.try_recv().expect("the value arrived"),
        "hello",
        "completion delivers the value to the waiting receiver"
    );
    assert_eq!(
        registry.complete(&token, "again".to_owned()),
        Err(WaitError::UnknownToken),
        "tokens are single-use: a duplicate complete is refused"
    );
    assert!(registry.unresolved().is_empty());
}

#[test]
fn an_unknown_token_reports_unknown_and_leaves_live_waits_alone() {
    let registry = WaitRegistry::new();
    let (token, mut receiver) = registry.create();
    assert_eq!(
        registry.complete("not-a-token", "x".to_owned()),
        Err(WaitError::UnknownToken)
    );
    assert_eq!(
        registry.unresolved(),
        vec![token.clone()],
        "a refused complete must not disturb the live wait"
    );
    registry
        .complete(&token, "still here".to_owned())
        .expect("the live wait was untouched");
    assert_eq!(
        receiver.try_recv().expect("the value arrived"),
        "still here"
    );
}

#[test]
fn cancel_kills_the_wait_and_its_token() {
    let registry = WaitRegistry::new();
    let (token, mut receiver) = registry.create();
    registry.cancel(&token);
    assert!(
        receiver.try_recv().is_err(),
        "a cancelled wait's receiver resolves dead rather than hanging"
    );
    assert_eq!(
        registry.complete(&token, "late".to_owned()),
        Err(WaitError::UnknownToken),
        "a cancelled token is dead to completion"
    );
    // Cancelling again is the normal cancel-races-completion no-op.
    registry.cancel(&token);
}

#[test]
fn tokens_are_distinct_and_unguessably_wide() {
    let registry = WaitRegistry::new();
    let mut receivers = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..64 {
        let (token, receiver) = registry.create();
        receivers.push(receiver);
        assert_eq!(token.len(), 32, "128 bits hex-encode to 32 characters");
        assert!(
            token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "tokens are lowercase hex"
        );
        assert!(seen.insert(token), "every token is unique");
    }
}

#[test]
fn the_registry_debug_shows_the_count_and_never_a_token() {
    let registry = WaitRegistry::new();
    let (token, _receiver) = registry.create();
    let rendered = format!("{registry:?}");
    assert_eq!(
        rendered, "WaitRegistry { unresolved: 1 }",
        "Debug reports the pending count"
    );
    assert!(
        !rendered.contains(&token),
        "a token in a log would let the log's reader answer the prompt"
    );
}

#[tokio::test]
async fn reconnect_resends_unresolved_waits_in_creation_order() {
    let registry = WaitRegistry::new();
    let (first, _first_receiver) = registry.create();
    let (second, _second_receiver) = registry.create();
    // The reconnecting client subscribes, then the session resends.
    let (frames, mut socket) = broadcast::channel(8);
    registry.resend_unresolved(&frames);
    assert_eq!(
        socket.recv().await.expect("the first resend arrives"),
        WaitFrame::Required { token: first },
        "resend replays the retained waits"
    );
    assert_eq!(
        socket.recv().await.expect("the second resend arrives"),
        WaitFrame::Required { token: second },
        "resend preserves creation order"
    );
}

#[test]
fn complete_input_response_runs_the_seam_before_the_wait_resumes() {
    let registry = WaitRegistry::new();
    let (token, mut receiver) = registry.create();
    let mut seam_ran = false;
    complete_input_response(&registry, &token, "typed".to_owned(), || {
        assert!(
            receiver.try_recv().is_err(),
            "the seam runs before the suspended call can see the text"
        );
        seam_ran = true;
    })
    .expect("a live wait completes");
    assert!(seam_ran, "the acceptance seam ran");
    assert_eq!(receiver.try_recv().expect("the value arrived"), "typed");
    assert_eq!(
        complete_input_response(&registry, &token, "again".to_owned(), || {}),
        Err(WaitError::UnknownToken),
        "the seam does not revive a consumed token"
    );
}

/// A fresh broker, registry, and channel with no subscribers.
fn broker_fixture() -> (
    Arc<SessionInputBroker>,
    Arc<WaitRegistry>,
    broadcast::Sender<WaitFrame>,
) {
    let registry = Arc::new(WaitRegistry::new());
    let (frames, _) = broadcast::channel(8);
    let broker = Arc::new(SessionInputBroker::new(
        Arc::clone(&registry),
        frames.clone(),
    ));
    (broker, registry, frames)
}

#[tokio::test]
async fn the_broker_announces_the_wait_and_resolves_with_the_operator_text() {
    let (broker, registry, frames) = broker_fixture();
    let mut socket = frames.subscribe();
    let call = spawn_tagged(mock_tag(), broker.wait("run".to_owned(), "chat".to_owned()));
    let token = required_token(&mut socket).await;
    assert_eq!(
        registry.unresolved(),
        vec![token.clone()],
        "the announced token names the retained wait"
    );
    registry
        .complete(&token, GNARLY.to_owned())
        .expect("the wait completes");
    let outcome = call
        .await
        .expect("the task joins")
        .expect("the broker answers");
    assert_eq!(
        outcome,
        InputOutcome::Text(GNARLY.to_owned()),
        "the operator's text rides back byte-exact"
    );
    assert!(
        matches!(
            socket.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ),
        "a completed wait dies silently: no cancelled frame follows"
    );
}

#[tokio::test]
async fn a_dropped_broker_future_removes_the_wait_and_emits_cancelled() {
    let (broker, registry, frames) = broker_fixture();
    let mut socket = frames.subscribe();
    let call = spawn_tagged(mock_tag(), broker.wait("run".to_owned(), "chat".to_owned()));
    let token = required_token(&mut socket).await;
    call.abort();
    let joined = call.await;
    assert!(
        joined.is_err_and(|error| error.is_cancelled()),
        "abort drops the suspended call"
    );
    assert!(
        registry.unresolved().is_empty(),
        "a dropped future may not leak its wait"
    );
    let frame = socket.recv().await.expect("the cancellation frame arrives");
    assert_eq!(
        frame,
        WaitFrame::Cancelled { token },
        "the client is told exactly which prompt died"
    );
}

#[tokio::test]
async fn a_registry_cancel_fails_the_broker_call_and_emits_cancelled() {
    let (broker, registry, frames) = broker_fixture();
    let mut socket = frames.subscribe();
    let call = spawn_tagged(mock_tag(), broker.wait("run".to_owned(), "chat".to_owned()));
    let token = required_token(&mut socket).await;
    registry.cancel(&token);
    let error = call
        .await
        .expect("the task joins")
        .expect_err("a cancelled wait fails the broker call");
    assert_eq!(error.to_string(), "the user-input wait was cancelled");
    let frame = socket.recv().await.expect("the cancellation frame arrives");
    assert_eq!(
        frame,
        WaitFrame::Cancelled { token },
        "cancellation is an outcome on the wire, not silence"
    );
}

#[tokio::test]
async fn a_user_input_effect_is_answered_when_the_registry_receives_the_text() {
    // The performer against a real engine effect: a section parked on
    // `user_input()` issues `Effect::UserInput`, the performer opens the
    // wait for it, the registry completes with the operator's text, and
    // the answer resumes the run to its result.
    let source = "---\nname: ask\ndescription: asks the operator\npromptforge: 0\n---\n\n\
                  # Ask\n\n## Only\n\n```lua\nreturn user_input()\n```\n";
    let (prompt, _parse_events) = Prompt::parse(source, "ask");
    let prompt = prompt.expect("the fixture prompt parses");
    let mut run = Run::new(
        Arc::new(prompt),
        "",
        RunContext::new("ask", 1, Timestamp::UNIX_EPOCH),
    );
    let Step::Pending { mut effects, .. } = run.step() else {
        panic!("the input wait leaves the run pending");
    };
    assert_eq!(effects.len(), 1, "one wait, one effect");
    let (id, _provenance, effect) = effects.remove(0);
    let Effect::UserInput { execution, section } = effect else {
        panic!("a parked user_input() issues a UserInput effect, got {effect:?}");
    };

    let (broker, registry, frames) = broker_fixture();
    let mut socket = frames.subscribe();
    let wait = spawn_tagged(mock_tag(), broker.wait(execution, section));
    let token = required_token(&mut socket).await;
    registry
        .complete(&token, "typed by the operator".to_owned())
        .expect("the wait completes");
    let answer = wait.await.expect("the wait task joins");

    run.resume(id, EffectAnswer::UserInput(answer));
    let Step::Done { result, .. } = run.step() else {
        panic!("the answered wait finishes the run");
    };
    let RunResult::Ok(text) = result else {
        panic!("the operator's text is the section's return value, got {result:?}");
    };
    assert_eq!(text, "typed by the operator");
    assert!(
        registry.unresolved().is_empty(),
        "the answered wait leaves nothing behind"
    );
}
