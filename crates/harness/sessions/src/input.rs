//! The user-input wait: the [`WaitRegistry`] of single-use wait tokens,
//! the session's input broker behind the script-side `user_input()` (the
//! harness's `InputPerformer`), and the producer seam that completes a
//! wait with the operator's text.
//!
//! An agent program asks its operator for input through the session's
//! input broker - session-supplied code, never advertised to a model. The
//! engine issues the call as a `UserInput` effect; the broker performs it
//! by registering a wait, announcing it with a durable
//! [`WaitFrame::Required`], and suspending on the wait's receiver until
//! the session completes the wait with the operator's answer or the wait
//! dies. A dying wait is an outcome, never silence: every path out of an
//! unresolved wait - the future dropped by a turn-cancel, the wait
//! cancelled out of the registry - removes the entry and pushes a durable
//! [`WaitFrame::Cancelled`], so a client never pins its input box to a
//! dead token. Unresolved waits are retained across socket loss and
//! re-announced on reconnect: sessions outlive sockets.
//!
//! The frames are harness data: the client that owns the socket
//! (Workshop's `/agents/ws`) renders each into its own protocol frame.

#[path = "input-tool.rs"]
mod tool;

use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::{broadcast, oneshot};

pub use tool::SessionInputBroker;

/// A user-input wait lifecycle notice, pushed on a session's wait
/// channel for its attached client to render.
///
/// `Required` announces an open wait: the client pins its input box to
/// the token and answers with the operator's text. `Cancelled` announces
/// a wait that died unresolved, so the client never holds a prompt
/// against a dead token - cancellation is an outcome, never silence.
///
/// Delivery is durable through the registry rather than the channel: the
/// [`WaitRegistry`] retains every unresolved wait and
/// [`resend_unresolved`](WaitRegistry::resend_unresolved) re-announces
/// them, so a push lost to a dead socket is repaired by the resent set -
/// a live wait reappears, and a cancelled one vanishes by its absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitFrame {
    /// A wait opened: the session wants operator input for `token`.
    Required {
        /// The single-use wait token the operator's answer must echo.
        token: String,
    },
    /// A wait died unresolved: the prompt for `token` is stale.
    Cancelled {
        /// The token whose wait is gone.
        token: String,
    },
}

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
    /// use harness_sessions::input::WaitRegistry;
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
    /// guessed by anything that has not seen the [`WaitFrame::Required`].
    ///
    /// # Examples
    /// ```
    /// use harness_sessions::input::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, mut receiver) = registry.create();
    /// registry.complete(&token, "hello".to_owned())?;
    /// assert_eq!(receiver.try_recv(), Ok("hello".to_owned()));
    /// # Ok::<(), harness_sessions::input::WaitError>(())
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
    /// use harness_sessions::input::{WaitError, WaitRegistry};
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, mut receiver) = registry.create();
    /// registry.complete(&token, "typed".to_owned())?;
    /// assert_eq!(receiver.try_recv(), Ok("typed".to_owned()));
    /// assert_eq!(
    ///     registry.complete(&token, "again".to_owned()),
    ///     Err(WaitError::UnknownToken),
    /// );
    /// # Ok::<(), harness_sessions::input::WaitError>(())
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
    /// racing the wait's own completion is normal, just as a chat
    /// cancel racing its `done` is.
    ///
    /// # Examples
    /// ```
    /// use harness_sessions::input::WaitRegistry;
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
    /// use harness_sessions::input::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, _receiver) = registry.create();
    /// assert_eq!(registry.unresolved(), vec![token]);
    /// ```
    #[must_use]
    pub fn unresolved(&self) -> Vec<String> {
        self.lock().iter().map(|wait| wait.token.clone()).collect()
    }

    /// Re-announces every unresolved wait to `frames` as a
    /// [`WaitFrame::Required`], in creation order.
    ///
    /// The reconnect half of the durable-delivery promise: a client that
    /// missed pushes rebuilds its prompt state from this resend - a live
    /// wait reappears, and a stale prompt vanishes by its absence.
    ///
    /// # Examples
    /// ```
    /// use harness_sessions::input::{WaitFrame, WaitRegistry};
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, _receiver) = registry.create();
    /// let (frames, mut socket) = tokio::sync::broadcast::channel(8);
    /// registry.resend_unresolved(&frames);
    /// assert_eq!(socket.try_recv()?, WaitFrame::Required { token });
    /// # Ok::<(), tokio::sync::broadcast::error::TryRecvError>(())
    /// ```
    pub fn resend_unresolved(&self, frames: &broadcast::Sender<WaitFrame>) {
        for token in self.unresolved() {
            // No receiver means the client vanished again between
            // subscribing and this resend; the registry still holds the
            // wait, so the next reconnect resends it once more.
            let _ = frames.send(WaitFrame::Required { token });
        }
    }
}

/// A [`WaitRegistry`] operation failed.
///
/// Exhaustive on purpose: a client matches every variant so a new
/// failure is a compile error at its render site, not a silent default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WaitError {
    /// No unresolved wait holds the token: never created, already
    /// completed (tokens are single-use), cancelled, or its suspended
    /// call dropped concurrently.
    #[error("no unresolved wait holds this token")]
    UnknownToken,
}

/// Completes the wait holding `token` with the operator's `text` without
/// recording anything: the producer side of an operator's answer.
///
/// The engine records the operator's text consumer-side, as a
/// `UserInput` event when the suspended `user_input` call resumes, so
/// a producer-side record here would double the event. The
/// `before_completion` seam runs after the response is accepted and
/// before the suspended call resumes.
///
/// # Errors
/// Returns [`WaitError::UnknownToken`] when no unresolved wait holds
/// `token`.
///
/// # Examples
/// ```
/// use harness_sessions::input::{WaitRegistry, complete_input_response};
///
/// let registry = WaitRegistry::new();
/// let (token, mut receiver) = registry.create();
/// let mut accepted = false;
/// complete_input_response(&registry, &token, "typed".to_owned(), || accepted = true)?;
/// assert!(accepted);
/// assert_eq!(receiver.try_recv(), Ok("typed".to_owned()));
/// # Ok::<(), harness_sessions::input::WaitError>(())
/// ```
pub fn complete_input_response(
    registry: &WaitRegistry,
    token: &str,
    text: String,
    before_completion: impl FnOnce(),
) -> Result<(), WaitError> {
    before_completion();
    registry.complete(token, text)
}

#[cfg(test)]
#[path = "input-tests.rs"]
mod tests;
