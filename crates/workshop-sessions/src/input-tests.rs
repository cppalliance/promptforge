use super::*;

use std::sync::Arc;

use promptforge_api::input::{InputBroker, InputOutcome};
use shared_promptforge_api::observe::Observation;

/// Hostile operator text covering the bytes most likely to be mangled
/// by an envelope or codec.
const GNARLY: &str = "line1\r\nline2 \"quoted\" {\"text\":\"decoy\"} \\slash \u{1F980}";

async fn required_token(socket: &mut broadcast::Receiver<InputFrame>) -> String {
    let frame = socket.recv().await.expect("a frame arrives");
    let InputFrame::Required { token } = frame else {
        panic!("expected input_required first, got {frame:?}");
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
        InputFrame::Required { token: first },
        "resend replays the retained waits"
    );
    assert_eq!(
        socket.recv().await.expect("the second resend arrives"),
        InputFrame::Required { token: second },
        "resend preserves creation order"
    );
}

#[derive(Default)]
struct RecordingObserver {
    inputs: Mutex<Vec<(String, String, String)>>,
}

impl RecordingObserver {
    fn inputs(&self) -> MutexGuard<'_, Vec<(String, String, String)>> {
        self.inputs.lock().expect("the recorder mutex stays usable")
    }
}

impl Observer for RecordingObserver {
    fn observe(&self, _execution: &str, _section: &str, _event: Observation) {}

    fn on_user_input(&self, execution: &str, section: &str, text: &str) {
        self.inputs()
            .push((execution.to_owned(), section.to_owned(), text.to_owned()));
    }
}

#[test]
fn on_user_input_fires_exactly_once_per_response_byte_exact_before_completion() {
    let registry = WaitRegistry::new();
    let observer = RecordingObserver::default();
    let (token, mut receiver) = registry.create();
    deliver_input_response(
        &observer,
        &registry,
        "run-1",
        "chat",
        InputResponse {
            token: token.clone(),
            text: GNARLY.to_owned(),
        },
    )
    .expect("a live wait completes");
    assert_eq!(
        receiver.try_recv().expect("the wait resumed"),
        GNARLY,
        "the completed value is the response text byte-exact"
    );
    assert_eq!(
        observer.inputs().as_slice(),
        &[("run-1".to_owned(), "chat".to_owned(), GNARLY.to_owned())],
        "exactly one byte-exact event per response"
    );
    // A duplicate response still records the operator's text - one
    // event per response - while the dead wait reports as the error.
    assert_eq!(
        deliver_input_response(
            &observer,
            &registry,
            "run-1",
            "chat",
            InputResponse {
                token,
                text: "again".to_owned(),
            },
        ),
        Err(WaitError::UnknownToken)
    );
    assert_eq!(
        observer.inputs().len(),
        2,
        "the event fires exactly once per response, even a stale one"
    );
}

/// A fresh broker, registry, and channel with no subscribers.
fn broker_fixture() -> (
    SessionInputBroker,
    Arc<WaitRegistry>,
    broadcast::Sender<InputFrame>,
) {
    let registry = Arc::new(WaitRegistry::new());
    let (frames, _) = broadcast::channel(8);
    let broker = SessionInputBroker::new(Arc::clone(&registry), frames.clone());
    (broker, registry, frames)
}

#[tokio::test]
async fn the_broker_announces_the_wait_and_resolves_with_the_operator_text() {
    let (broker, registry, frames) = broker_fixture();
    let mut socket = frames.subscribe();
    let call = tokio::spawn(async move { broker.user_input("run", "chat").await });
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
        "a completed wait dies silently: no input_cancelled follows"
    );
}

#[tokio::test]
async fn a_dropped_broker_future_removes_the_wait_and_emits_input_cancelled() {
    let (broker, registry, frames) = broker_fixture();
    let mut socket = frames.subscribe();
    let call = tokio::spawn(async move { broker.user_input("run", "chat").await });
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
        InputFrame::Cancelled { token },
        "the SPA is told exactly which prompt died"
    );
}

#[tokio::test]
async fn a_registry_cancel_fails_the_broker_call_and_emits_input_cancelled() {
    let (broker, registry, frames) = broker_fixture();
    let mut socket = frames.subscribe();
    let call = tokio::spawn(async move { broker.user_input("run", "chat").await });
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
        InputFrame::Cancelled { token },
        "cancellation is an outcome on the wire, not silence"
    );
}
