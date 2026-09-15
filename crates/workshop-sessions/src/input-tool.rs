//! The session's input broker: suspends an agent program until its
//! operator answers, guarded so a dying wait is an outcome, never
//! silence.

use std::sync::Arc;

use promptforge_api::input::{InputBroker, InputError, InputOutcome};
use tokio::sync::broadcast;

use workshop_protocol::InputFrame;

use super::WaitRegistry;

/// Guarantees a dying wait is an outcome, not silence: unless disarmed by
/// a delivered value, dropping the guard removes the wait from the
/// registry and pushes `input_cancelled` for its token. The broker future
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

/// The session's wait registry behind the generic input-broker interface:
/// the adapter the unified runtime's script-side `user_input()` suspends
/// on.
///
/// One broker per run: `user_input` opens a wait in the session's
/// [`WaitRegistry`], announces it with the durable `input_required` frame,
/// and suspends on the receiver until the session delivers the operator's
/// answer or the wait dies. A dying wait is an outcome, never silence:
/// a future dropped by a
/// turn-cancel removes the entry and pushes `input_cancelled`, so the SPA
/// never pins its input box to a dead token.
///
/// # Examples
/// ```
/// use std::sync::Arc;
///
/// use workshop_sessions::{SessionInputBroker, WaitRegistry};
///
/// let (frames, _receiver) = tokio::sync::broadcast::channel(8);
/// let broker = SessionInputBroker::new(Arc::new(WaitRegistry::new()), frames);
/// # drop(broker);
/// ```
#[derive(Debug)]
pub struct SessionInputBroker {
    /// The session's wait registry, shared with the session loop that
    /// completes and cancels waits.
    registry: Arc<WaitRegistry>,
    /// Where `input_required` and `input_cancelled` frames are pushed;
    /// the session's socket loop forwards them to the SPA.
    frames: broadcast::Sender<InputFrame>,
}

impl SessionInputBroker {
    /// Builds the broker over the session's wait registry and frame sender.
    ///
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    ///
    /// use workshop_sessions::{SessionInputBroker, WaitRegistry};
    ///
    /// let registry = Arc::new(WaitRegistry::new());
    /// let (frames, _receiver) = tokio::sync::broadcast::channel(8);
    /// let _broker = SessionInputBroker::new(registry, frames);
    /// ```
    #[must_use]
    pub fn new(registry: Arc<WaitRegistry>, frames: broadcast::Sender<InputFrame>) -> Self {
        Self { registry, frames }
    }
}

#[async_trait::async_trait]
impl InputBroker for SessionInputBroker {
    /// Opens a wait, announces it, and suspends until it resolves.
    ///
    /// On cancellation - the future dropped mid-await, or the wait
    /// cancelled out of the registry - the drop guard removes the wait and
    /// pushes `input_cancelled`, so no path leaks a wait or a stale
    /// prompt. A wait cancelled out of the registry resolves here as the
    /// broker's failure policy.
    ///
    /// # Errors
    /// Returns an [`InputError`] when the wait dies before the operator
    /// answers.
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> Result<InputOutcome, InputError> {
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
                Ok(InputOutcome::Text(text))
            }
            // The sender died without a value: the wait was cancelled out
            // of the registry. The still-armed guard pushes
            // `input_cancelled` on scope exit, so this path clears the
            // SPA prompt too.
            Err(_) => Err(InputError::message("the user-input wait was cancelled")),
        }
    }
}
