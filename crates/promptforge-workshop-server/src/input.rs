//! The user-input wait: the [`WaitRegistry`] of single-use wait tokens,
//! the Workshop's `user_input` tool, and the `input_response` producer
//! that completes a wait.
//!
//! An agent program asks its operator for input by calling the
//! `user_input` tool - session-supplied code, never advertised to a
//! model. Its `call()` registers a wait, announces it with a durable
//! `input_required` frame, and suspends on the wait's receiver until the
//! session delivers the operator's answer ([`deliver_input_response`]) or
//! the wait dies. A dying wait is an outcome, never silence: every path
//! out of an unresolved wait - the future dropped by a turn-cancel, the
//! wait cancelled out of the registry - removes the entry and pushes a
//! durable `input_cancelled` frame, so the SPA never pins its input box
//! to a dead token. Unresolved waits are retained across socket loss and
//! re-announced on reconnect: sessions outlive sockets.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use promptforge_core_support::observe::Observer;
use promptforge_tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};
use tokio::sync::{broadcast, oneshot};

use crate::protocol::{InputFrame, InputResponse};

/// One unresolved wait: its single-use token, and the sender that resumes
/// the suspended `user_input` call with the operator's text.
struct Wait {
    /// The unguessable token an `input_response` must echo.
    token: String,
    /// Resumes the suspended call; dropping it without a value resolves
    /// the call as cancelled.
    sender: oneshot::Sender<String>,
}

/// The registry of unresolved user-input waits, keyed by single-use
/// cryptographic tokens.
///
/// [`create`](Self::create) opens a wait and returns its token beside the
/// receiving half; [`complete`](Self::complete) resolves the wait with the
/// operator's text and consumes the token; [`cancel`](Self::cancel) kills
/// it. Unresolved waits are retained - sessions outlive sockets - and
/// [`resend_unresolved`](Self::resend_unresolved) re-announces them to a
/// reconnecting client in creation order.
#[derive(Default)]
pub struct WaitRegistry {
    /// The unresolved waits in creation order. A `Vec` rather than a map:
    /// a session holds at most a handful of waits (in the gate, one), and
    /// creation order is exactly the resend order reconnect needs.
    waits: Mutex<Vec<Wait>>,
}

/// Shows the unresolved count, never the tokens: a token in a log would
/// let whoever reads the log answer someone else's prompt.
impl fmt::Debug for WaitRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WaitRegistry")
            .field("unresolved", &self.lock().len())
            .finish()
    }
}

impl WaitRegistry {
    /// Opens an empty registry.
    ///
    /// # Examples
    /// ```
    /// use promptforge_workshop_server::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// assert!(registry.unresolved().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry lock. Zone two: a peer that panicked mid-mutation
    /// cannot wedge the process, and the recovered list is still
    /// consistent because every mutation is one push, remove, or retain.
    fn lock(&self) -> MutexGuard<'_, Vec<Wait>> {
        self.waits.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens a wait: returns its fresh single-use token and the receiver
    /// that resolves with the operator's text.
    ///
    /// The token is 128 bits from the OS-seeded cryptographic RNG
    /// (`rand::rng`, a ChaCha-based CSPRNG), hex-encoded, so it cannot be
    /// guessed by anything that has not seen the `input_required` frame.
    ///
    /// # Examples
    /// ```
    /// use promptforge_workshop_server::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, mut receiver) = registry.create();
    /// registry.complete(&token, "hello".to_owned())?;
    /// assert_eq!(receiver.try_recv(), Ok("hello".to_owned()));
    /// # Ok::<(), promptforge_workshop_server::WaitError>(())
    /// ```
    #[must_use]
    pub fn create(&self) -> (String, oneshot::Receiver<String>) {
        use rand::Rng as _;
        let mut rng = rand::rng();
        let token = format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>());
        let (sender, receiver) = oneshot::channel();
        self.lock().push(Wait {
            token: token.clone(),
            sender,
        });
        (token, receiver)
    }

    /// Resolves the wait holding `token` with the operator's text,
    /// consuming the token: a second `complete` of the same token fails.
    ///
    /// # Errors
    /// Returns [`WaitError::UnknownToken`] when no unresolved wait holds
    /// `token` - never created, already completed, cancelled, or its
    /// suspended call dropped concurrently. The undelivered `value` is
    /// discarded with the error: a dead wait has no consumer left.
    ///
    /// # Examples
    /// ```
    /// use promptforge_workshop_server::{WaitError, WaitRegistry};
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, mut receiver) = registry.create();
    /// registry.complete(&token, "typed".to_owned())?;
    /// assert_eq!(receiver.try_recv(), Ok("typed".to_owned()));
    /// assert_eq!(
    ///     registry.complete(&token, "again".to_owned()),
    ///     Err(WaitError::UnknownToken),
    /// );
    /// # Ok::<(), promptforge_workshop_server::WaitError>(())
    /// ```
    pub fn complete(&self, token: &str, value: String) -> Result<(), WaitError> {
        let wait = {
            let mut waits = self.lock();
            let index = waits
                .iter()
                .position(|wait| wait.token == token)
                .ok_or(WaitError::UnknownToken)?;
            waits.remove(index)
        };
        wait.sender.send(value).map_err(|_| WaitError::UnknownToken)
    }

    /// Kills the wait holding `token`: the entry is removed and the
    /// suspended call resolves as cancelled.
    ///
    /// Cancelling a token with no wait is a no-op, because a cancel
    /// racing the wait's own completion is normal, exactly as a chat
    /// cancel racing its `done` is.
    ///
    /// # Examples
    /// ```
    /// use promptforge_workshop_server::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, mut receiver) = registry.create();
    /// registry.cancel(&token);
    /// assert!(receiver.try_recv().is_err(), "the wait resolves as dead");
    /// assert!(registry.unresolved().is_empty());
    /// ```
    pub fn cancel(&self, token: &str) {
        self.lock().retain(|wait| wait.token != token);
    }

    /// Returns the unresolved wait tokens in creation order.
    ///
    /// This is the retained state behind reconnect resend and the
    /// leaked-wait assertion in session teardown tests.
    ///
    /// # Examples
    /// ```
    /// use promptforge_workshop_server::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, _receiver) = registry.create();
    /// assert_eq!(registry.unresolved(), vec![token]);
    /// ```
    #[must_use]
    pub fn unresolved(&self) -> Vec<String> {
        self.lock().iter().map(|wait| wait.token.clone()).collect()
    }

    /// Re-announces every unresolved wait to `frames` as an
    /// `input_required` frame, in creation order.
    ///
    /// The reconnect half of the durable-delivery promise: a client that
    /// missed pushes rebuilds its prompt state from this resend - a live
    /// wait reappears, and a stale prompt vanishes by its absence.
    ///
    /// # Examples
    /// ```
    /// use promptforge_workshop_server::{InputFrame, WaitRegistry};
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, _receiver) = registry.create();
    /// let (frames, mut socket) = tokio::sync::broadcast::channel(8);
    /// registry.resend_unresolved(&frames);
    /// assert_eq!(socket.try_recv()?, InputFrame::Required { token });
    /// # Ok::<(), tokio::sync::broadcast::error::TryRecvError>(())
    /// ```
    pub fn resend_unresolved(&self, frames: &broadcast::Sender<InputFrame>) {
        for token in self.unresolved() {
            // No receiver means the client vanished again between
            // subscribing and this resend; the registry still holds the
            // wait, so the next reconnect resends it once more.
            let _ = frames.send(InputFrame::Required { token });
        }
    }
}

/// A [`WaitRegistry`] operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WaitError {
    /// No unresolved wait holds the token: never created, already
    /// completed (tokens are single-use), cancelled, or its suspended
    /// call dropped concurrently.
    #[error("no unresolved wait holds this token")]
    UnknownToken,
}

/// Fires `on_user_input` for an arrived `input_response`, byte-exact,
/// then completes the wait its token names.
///
/// This is the producer the session calls when the SPA answers a prompt.
/// The event fires exactly once per response, before completion and
/// regardless of whether the token still names a live wait: the
/// operator's text is history the relaunched agent rebuilds context
/// from, so a response racing a turn-cancel records its text even though
/// the wait it aimed at is gone.
///
/// # Errors
/// Returns [`WaitError::UnknownToken`] when no unresolved wait holds the
/// response's token; the `on_user_input` event has fired regardless.
///
/// # Examples
/// ```
/// use promptforge_core_support::observe::NullObserver;
/// use promptforge_workshop_server::{InputResponse, WaitRegistry, deliver_input_response};
///
/// let registry = WaitRegistry::new();
/// let (token, mut receiver) = registry.create();
/// deliver_input_response(
///     &NullObserver::default(),
///     &registry,
///     "run",
///     "chat",
///     InputResponse { token, text: "hello".to_owned() },
/// )?;
/// assert_eq!(receiver.try_recv(), Ok("hello".to_owned()));
/// # Ok::<(), promptforge_workshop_server::WaitError>(())
/// ```
pub fn deliver_input_response(
    observer: &dyn Observer,
    registry: &WaitRegistry,
    execution: &str,
    section: &str,
    response: InputResponse,
) -> Result<(), WaitError> {
    observer.on_user_input(execution, section, &response.text);
    registry.complete(&response.token, response.text)
}

/// The Workshop's `user_input` tool: suspends an agent program until its
/// operator types into the session's input box.
///
/// A host-primitive [`Tool`] the session constructs per agent session -
/// it is never advertised to a model (the agent driver advertises only
/// the aliases a `models.chat` call names, and host primitives are
/// excluded from that set), and only the agent program itself calls it.
/// `call()` opens a wait in the session's [`WaitRegistry`], pushes the
/// `input_required` frame itself, and suspends until the wait resolves;
/// `run_agent` has no user-input awareness because this tool is the
/// caller's own code.
///
/// The output is **trusted and structured**: a JSON object with `text`
/// (the operator's input, byte-exact - the operator is not an attacker of
/// their own session, so no nonce envelope ever wraps it) and `images`
/// (present and always empty until SPA attachments land). The session
/// binds this tool with the structured output kind, so the object resumes
/// into Lua as a table - `result.text`, `result.images` - through the
/// serde boundary; structured output stays restricted to trusted tools.
///
/// # Examples
/// ```
/// use std::sync::Arc;
///
/// use promptforge_tools::Tool;
/// use promptforge_workshop_server::{UserInputTool, WaitRegistry};
///
/// let (frames, _receiver) = tokio::sync::broadcast::channel(8);
/// let tool = UserInputTool::new(Arc::new(WaitRegistry::new()), frames);
/// assert_eq!(tool.wire_name(), "user_input");
/// ```
#[derive(Debug)]
pub struct UserInputTool {
    /// The session's wait registry, shared with the session loop that
    /// completes and cancels waits.
    registry: Arc<WaitRegistry>,
    /// Where `input_required` and `input_cancelled` frames are pushed;
    /// the session's socket loop forwards them to the SPA.
    frames: broadcast::Sender<InputFrame>,
}

impl UserInputTool {
    /// Builds the tool over the session's wait registry and frame sender.
    ///
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    ///
    /// use promptforge_workshop_server::{UserInputTool, WaitRegistry};
    ///
    /// let registry = Arc::new(WaitRegistry::new());
    /// let (frames, _receiver) = tokio::sync::broadcast::channel(8);
    /// let _tool = UserInputTool::new(registry, frames);
    /// ```
    #[must_use]
    pub fn new(registry: Arc<WaitRegistry>, frames: broadcast::Sender<InputFrame>) -> Self {
        Self { registry, frames }
    }
}

/// Guarantees a dying wait is an outcome, not silence: unless disarmed by
/// a delivered value, dropping the guard removes the wait from the
/// registry and pushes `input_cancelled` for its token. The tool future
/// is dropped by the shared dispatch's cancel race on turn-cancel, so
/// this guard is what keeps a cancelled turn from leaking its wait or
/// leaving the SPA prompting against a dead token.
struct WaitGuard {
    /// The registry the wait entry is removed from.
    registry: Arc<WaitRegistry>,
    /// Where the `input_cancelled` frame is pushed.
    frames: broadcast::Sender<InputFrame>,
    /// The dying wait's token.
    token: String,
    /// Cleared when the wait resolved with a value; the guard then does
    /// nothing, because `complete` already consumed the entry.
    armed: bool,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // On the registry-cancel path the entry is already gone and this
        // is a no-op; on the dropped-future path it is the removal.
        self.registry.cancel(&self.token);
        // No receiver means no socket is attached; the reconnect resend
        // repairs the SPA anyway, because this wait is absent from the
        // resent set.
        let _ = self.frames.send(InputFrame::Cancelled {
            token: std::mem::take(&mut self.token),
        });
    }
}

#[async_trait::async_trait]
impl Tool for UserInputTool {
    fn id(&self) -> ToolId {
        ToolId::from_validated("workshop", "user_input")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "user_input"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Waits for the workshop operator to type into the session's input box."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    /// Opens a wait, announces it, and suspends until it resolves.
    ///
    /// Arguments are ignored: the tool takes none. On cancellation - the
    /// future dropped mid-await, or the wait cancelled out of the
    /// registry - the drop guard removes the wait and pushes
    /// `input_cancelled`, so no path leaks a wait or a stale prompt.
    ///
    /// # Errors
    /// Returns a [`ToolErrorKind::Cancelled`] error when the wait dies
    /// before the operator answers.
    async fn call(&self, _args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let (token, receiver) = self.registry.create();
        let mut guard = WaitGuard {
            registry: Arc::clone(&self.registry),
            frames: self.frames.clone(),
            token,
            armed: true,
        };
        // No receiver means no socket is attached right now. Not a
        // failure: the registry retains the wait and the session resends
        // it on reconnect, so the lost push is repaired.
        let _ = self.frames.send(InputFrame::Required {
            token: guard.token.clone(),
        });
        match receiver.await {
            Ok(text) => {
                guard.armed = false;
                let table = serde_json::json!({ "text": text, "images": [] });
                Ok(ToolOutput::trusted(table.to_string()))
            }
            // The sender died without a value: the wait was cancelled out
            // of the registry. The still-armed guard pushes
            // `input_cancelled` on scope exit, so this path clears the
            // SPA prompt too.
            Err(_) => Err(ToolError::message("the user-input wait was cancelled")
                .with_kind(ToolErrorKind::Cancelled)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use promptforge_core_support::observe::Observation;
    use promptforge_tools::OutputTrust;

    /// Hostile operator text - CRLF, quotes, JSON braces, a backslash,
    /// and a multi-byte scalar - so byte-exactness is proven on the bytes
    /// most likely to be mangled by an envelope or a codec.
    const GNARLY: &str = "line1\r\nline2 \"quoted\" {\"text\":\"decoy\"} \\slash \u{1F980}";

    /// A tool over a fresh registry and channel; the channel's initial
    /// receiver is dropped, so tests start with zero subscribers.
    fn tool_fixture() -> (
        UserInputTool,
        Arc<WaitRegistry>,
        broadcast::Sender<InputFrame>,
    ) {
        let registry = Arc::new(WaitRegistry::new());
        let (frames, _) = broadcast::channel(8);
        let tool = UserInputTool::new(Arc::clone(&registry), frames.clone());
        (tool, registry, frames)
    }

    /// Waits for a spawned call to register its wait, without a socket.
    async fn registered_token(registry: &WaitRegistry) -> String {
        for _ in 0..1024 {
            if let Some(token) = registry.unresolved().first().cloned() {
                return token;
            }
            tokio::task::yield_now().await;
        }
        panic!("the tool call never registered its wait");
    }

    /// Receives the next frame and unwraps the `input_required` token.
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
    async fn the_tool_emits_input_required_carrying_its_wait_token() {
        let (tool, registry, frames) = tool_fixture();
        let mut socket = frames.subscribe();
        let call = tokio::spawn(async move { tool.call(serde_json::json!({})).await });
        let token = required_token(&mut socket).await;
        assert_eq!(
            registry.unresolved(),
            vec![token.clone()],
            "the announced token names the retained wait"
        );
        registry
            .complete(&token, "done".to_owned())
            .expect("the wait completes");
        let output = call
            .await
            .expect("the task joins")
            .expect("the call succeeds");
        assert_eq!(output.trust(), OutputTrust::Trusted);
    }

    #[tokio::test]
    async fn the_resumed_output_is_a_trusted_table_with_byte_exact_text_and_empty_images() {
        let (tool, registry, frames) = tool_fixture();
        let mut socket = frames.subscribe();
        let call = tokio::spawn(async move { tool.call(serde_json::json!({})).await });
        let token = required_token(&mut socket).await;
        registry
            .complete(&token, GNARLY.to_owned())
            .expect("the wait completes");
        let output = call
            .await
            .expect("the task joins")
            .expect("the call succeeds");
        assert_eq!(
            output.trust(),
            OutputTrust::Trusted,
            "operator input is first-party: no nonce envelope may wrap it"
        );
        let table: serde_json::Value =
            serde_json::from_str(output.text()).expect("a structured tool returns JSON");
        assert_eq!(
            table["text"].as_str().expect("text is a string"),
            GNARLY,
            "result.text is the SPA text byte-exact and envelope-free"
        );
        assert_eq!(
            table["images"],
            serde_json::json!([]),
            "result.images is present and empty in the gate"
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
    async fn dropping_the_tool_future_removes_the_wait_and_emits_input_cancelled() {
        let (tool, registry, frames) = tool_fixture();
        let mut socket = frames.subscribe();
        let call = tokio::spawn(async move { tool.call(serde_json::json!({})).await });
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
    async fn a_registry_cancel_fails_the_call_as_cancelled_and_emits_input_cancelled() {
        let (tool, registry, frames) = tool_fixture();
        let mut socket = frames.subscribe();
        let call = tokio::spawn(async move { tool.call(serde_json::json!({})).await });
        let token = required_token(&mut socket).await;
        registry.cancel(&token);
        let error = call
            .await
            .expect("the task joins")
            .expect_err("a cancelled wait fails the call");
        assert_eq!(error.kind(), ToolErrorKind::Cancelled);
        let frame = socket.recv().await.expect("the cancellation frame arrives");
        assert_eq!(
            frame,
            InputFrame::Cancelled { token },
            "cancellation is an outcome on the wire, not silence"
        );
    }

    #[tokio::test]
    async fn a_disconnected_socket_does_not_cancel_the_wait() {
        let (tool, registry, frames) = tool_fixture();
        // No subscriber exists at all: the session's socket is gone.
        drop(frames);
        let call = tokio::spawn(async move { tool.call(serde_json::json!({})).await });
        let token = registered_token(&registry).await;
        assert_eq!(
            registry.unresolved(),
            vec![token.clone()],
            "the wait outlives the absent socket"
        );
        registry
            .complete(&token, "typed after reconnect".to_owned())
            .expect("the retained wait still completes");
        let output = call
            .await
            .expect("the task joins")
            .expect("the call succeeds");
        let table: serde_json::Value =
            serde_json::from_str(output.text()).expect("a structured tool returns JSON");
        assert_eq!(table["text"], "typed after reconnect");
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

    /// Records every `on_user_input` report for the producer tests.
    #[derive(Default)]
    struct RecordingObserver {
        inputs: Mutex<Vec<(String, String, String)>>,
    }

    impl Observer for RecordingObserver {
        fn observe(&self, _execution: &str, _section: &str, _event: Observation) {}

        fn on_user_input(&self, execution: &str, section: &str, text: &str) {
            self.inputs
                .lock()
                .expect("the recorder mutex stays usable")
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
            observer
                .inputs
                .lock()
                .expect("the recorder mutex stays usable")
                .as_slice(),
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
            observer
                .inputs
                .lock()
                .expect("the recorder mutex stays usable")
                .len(),
            2,
            "the event fires exactly once per response, even a stale one"
        );
    }
}
