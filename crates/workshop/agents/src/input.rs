//! The user-input wait: the [`WaitRegistry`] of single-use wait tokens,
//! the conversation's input broker (the Harness's `InputBroker`) handed
//! to its run, and the producer seam that completes a wait with the
//! operator's text.
//!
//! An agent prompt asks its operator for input through a capability that
//! holds the run's broker: the `promptforge/user-input` capability's
//! `input.ask()` calls its ask tool, which waits on the broker. The
//! broker performs each wait by registering it, announcing it with a
//! durable [`WaitFrame::Required`], and suspending on the wait's receiver
//! until the conversation completes the wait with the operator's answer
//! or the wait dies. A dying wait is an outcome, never silence: every
//! path out of an unresolved wait - the future dropped by a close, the
//! wait cancelled out of the registry - removes the entry and pushes a
//! durable [`WaitFrame::Cancelled`], so a client never pins its input box
//! to a dead token. Unresolved waits are retained across socket loss and
//! re-announced on reconnect: conversations outlive sockets.
//!
//! The frames are Workshop data: the agent socket renders each into its
//! own protocol frame.

#[path = "input-tool.rs"]
mod tool;

use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::oneshot;

pub use tool::SessionInputBroker;

/// A user-input wait lifecycle notice, pushed on a conversation's wait
/// channel for its attached client to render.
///
/// `Required` announces an open wait: the client pins its input box to
/// the token and answers with the operator's text. `Cancelled` announces
/// a wait that died unresolved, so the client never holds a prompt
/// against a dead token - cancellation is an outcome, never silence.
///
/// Delivery is durable through the registry rather than the channel: the
/// [`WaitRegistry`] retains every unresolved wait, and the agent socket
/// re-announces them from
/// [`Conversation::unresolved_waits()`](crate::Conversation::unresolved_waits)
/// on reconnect and after lag, so a push lost to a dead socket is
/// repaired by the resent set - a live wait reappears, and a cancelled
/// one vanishes by its absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitFrame {
    /// A wait opened: the conversation wants operator input for `token`.
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
/// the suspended ask with the operator's text.
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
/// it. Unresolved waits are retained - conversations outlive sockets -
/// and the agent socket re-announces them in creation order from
/// [`Conversation::unresolved_waits()`](crate::Conversation::unresolved_waits)
/// on reconnect and after lag.
#[derive(Default)]
pub struct WaitRegistry {
    /// The unresolved waits in creation order. A `Vec` rather than a map:
    /// a conversation holds at most a handful of waits (in the chat, one),
    /// and creation order is exactly the resend order reconnect needs.
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
    /// use workshop_agents::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// assert!(registry.unresolved().is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry lock. A peer that panicked mid-mutation cannot
    /// wedge the process, and the recovered list is still consistent
    /// because every mutation is one push, remove, or retain.
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
    /// use workshop_agents::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, mut receiver) = registry.create();
    /// registry.complete(&token, "hello".to_owned())?;
    /// assert_eq!(receiver.try_recv(), Ok("hello".to_owned()));
    /// # Ok::<(), workshop_agents::WaitError>(())
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
    /// use workshop_agents::{WaitError, WaitRegistry};
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, mut receiver) = registry.create();
    /// registry.complete(&token, "typed".to_owned())?;
    /// assert_eq!(receiver.try_recv(), Ok("typed".to_owned()));
    /// assert_eq!(
    ///     registry.complete(&token, "again".to_owned()),
    ///     Err(WaitError::UnknownToken),
    /// );
    /// # Ok::<(), workshop_agents::WaitError>(())
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
    /// racing the wait's own completion is normal.
    ///
    /// # Examples
    /// ```
    /// use workshop_agents::WaitRegistry;
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
    /// leaked-wait assertion in conversation teardown tests.
    ///
    /// # Examples
    /// ```
    /// use workshop_agents::WaitRegistry;
    ///
    /// let registry = WaitRegistry::new();
    /// let (token, _receiver) = registry.create();
    /// assert_eq!(registry.unresolved(), vec![token]);
    /// ```
    #[must_use]
    pub fn unresolved(&self) -> Vec<String> {
        self.lock().iter().map(|wait| wait.token.clone()).collect()
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
/// The run records the operator's text consumer-side, as the ask tool's
/// answer record and its `ToolResult` event when the suspended ask
/// resumes, so a producer-side record here would double it. The
/// `before_completion` seam runs after the response is accepted and
/// before the suspended ask resumes.
///
/// # Errors
/// Returns [`WaitError::UnknownToken`] when no unresolved wait holds
/// `token`.
///
/// # Examples
/// ```
/// use workshop_agents::{WaitRegistry, complete_input_response};
///
/// let registry = WaitRegistry::new();
/// let (token, mut receiver) = registry.create();
/// let mut accepted = false;
/// complete_input_response(&registry, &token, "typed".to_owned(), || accepted = true)?;
/// assert!(accepted);
/// assert_eq!(receiver.try_recv(), Ok("typed".to_owned()));
/// # Ok::<(), workshop_agents::WaitError>(())
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
