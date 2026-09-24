//! The session's input broker: the harness's [`InputPerformer`], which
//! suspends an agent program's `UserInput` effect until its operator
//! answers, guarded so a dying wait is an outcome, never silence.

use std::sync::Arc;

use harness_runner::performers::{BoxFuture, InputPerformer};
use promptforge::input::{InputError, InputOutcome};
use tokio::sync::broadcast;

use super::{WaitFrame, WaitRegistry};

/// Turns a dying wait into an outcome: unless disarmed by a delivered
/// value, dropping the guard removes the wait from the registry and
/// pushes [`WaitFrame::Cancelled`] for its token. The performer's future
/// is aborted by the effect loop on cancel, so this guard is what keeps a
/// cancelled turn from leaking its wait or leaving the client prompting
/// against a dead token.
struct WaitGuard {
    /// The registry the wait entry is removed from.
    registry: Arc<WaitRegistry>,
    /// Where the cancelled frame is pushed.
    frames: broadcast::Sender<WaitFrame>,
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
        // repairs the client anyway, because this wait is absent from the
        // resent set.
        let _ = self.frames.send(WaitFrame::Cancelled {
            token: std::mem::take(&mut self.token),
        });
    }
}

/// The session's wait registry behind the harness's input performer:
/// what the engine's `UserInput` effect, issued for the script-side
/// `user_input()`, suspends on.
///
/// One broker per session: each [`wait`](InputPerformer::wait) opens a
/// wait in the session's [`WaitRegistry`], announces it with the durable
/// [`WaitFrame::Required`], and suspends on the receiver until the
/// session delivers the operator's answer or the wait dies. A dying wait
/// is an outcome, never silence: a future dropped by a turn-cancel
/// removes the entry and pushes [`WaitFrame::Cancelled`], so the client
/// never pins its input box to a dead token.
///
/// # Examples
/// ```
/// use std::sync::Arc;
///
/// use harness_sessions::input::{SessionInputBroker, WaitRegistry};
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
    /// Where the required and cancelled frames are pushed; the session's
    /// socket loop forwards them to its client.
    frames: broadcast::Sender<WaitFrame>,
}

impl SessionInputBroker {
    /// Builds the broker over the session's wait registry and frame sender.
    ///
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    ///
    /// use harness_sessions::input::{SessionInputBroker, WaitRegistry};
    ///
    /// let registry = Arc::new(WaitRegistry::new());
    /// let (frames, _receiver) = tokio::sync::broadcast::channel(8);
    /// let _broker = SessionInputBroker::new(registry, frames);
    /// ```
    #[must_use]
    pub fn new(registry: Arc<WaitRegistry>, frames: broadcast::Sender<WaitFrame>) -> Self {
        Self { registry, frames }
    }
}

impl InputPerformer for SessionInputBroker {
    /// Opens a wait, announces it, and suspends until it resolves.
    ///
    /// On cancellation - the future dropped mid-await, or the wait
    /// cancelled out of the registry - the drop guard removes the wait and
    /// pushes [`WaitFrame::Cancelled`], so no path leaks a wait or a stale
    /// prompt. A wait cancelled out of the registry resolves here as the
    /// broker's failure policy: an [`InputError`] the engine raises at the
    /// Lua call site.
    fn wait(
        &self,
        _execution: String,
        _section: String,
    ) -> BoxFuture<Result<InputOutcome, InputError>> {
        let registry = Arc::clone(&self.registry);
        let frames = self.frames.clone();
        Box::pin(async move {
            let (token, receiver) = registry.create();
            let mut guard = WaitGuard {
                registry,
                frames,
                token,
                armed: true,
            };
            // No receiver means no socket is attached right now. Not a
            // failure: the registry retains the wait and the session
            // resends it on reconnect, so the lost push is repaired.
            let _ = guard.frames.send(WaitFrame::Required {
                token: guard.token.clone(),
            });
            match receiver.await {
                Ok(text) => {
                    guard.armed = false;
                    Ok(InputOutcome::Text(text))
                }
                // The sender died without a value: the wait was cancelled
                // out of the registry. The still-armed guard pushes the
                // cancelled frame on scope exit, so this path clears the
                // client's prompt too.
                Err(_) => Err(InputError::message("the user-input wait was cancelled")),
            }
        })
    }
}
