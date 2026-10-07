//! The conversation's input broker: the Harness's [`InputBroker`], which
//! suspends an agent prompt's wait for input until its operator answers,
//! guarded so a dying wait is an outcome, never silence.

use std::sync::Arc;

use async_trait::async_trait;
use plugin_user_input::InputBroker;
use promptforge::tools::{ToolError, ToolErrorKind};
use tokio::sync::broadcast;

use super::{WaitFrame, WaitRegistry};

/// Turns a dying wait into an outcome: unless disarmed by a delivered
/// value, dropping the guard removes the wait from the registry and
/// pushes [`WaitFrame::Cancelled`] for its token. The Harness drops the
/// performer's future when it drops the effect on a close, so this guard
/// is what keeps a cancelled run from leaking its wait or leaving the
/// client prompting against a dead token.
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

/// The conversation's wait registry behind the Harness's input broker:
/// what the `user-input` ask tool, called by the script-side
/// `input.ask()`, suspends on.
///
/// One broker per conversation: each [`wait`](InputBroker::wait) opens a
/// wait in the conversation's [`WaitRegistry`], announces it with the
/// durable [`WaitFrame::Required`], and suspends on the receiver until
/// the conversation delivers the operator's answer or the wait dies. A
/// dying wait is an outcome, never silence: a future dropped by a close
/// removes the entry and pushes [`WaitFrame::Cancelled`], so the client
/// never pins its input box to a dead token.
#[derive(Debug)]
pub struct SessionInputBroker {
    /// The conversation's wait registry, shared with the conversation
    /// that completes and cancels waits.
    registry: Arc<WaitRegistry>,
    /// Where the required and cancelled frames are pushed; the agent
    /// socket forwards them to its client.
    frames: broadcast::Sender<WaitFrame>,
}

impl SessionInputBroker {
    /// Builds the broker over the conversation's wait registry and frame
    /// sender.
    #[must_use]
    pub fn new(registry: Arc<WaitRegistry>, frames: broadcast::Sender<WaitFrame>) -> Self {
        Self { registry, frames }
    }
}

#[async_trait]
impl InputBroker for SessionInputBroker {
    /// Opens a wait, announces it, and suspends until it resolves.
    ///
    /// On cancellation - the future dropped mid-await, or the wait
    /// cancelled out of the registry - the drop guard removes the wait and
    /// pushes [`WaitFrame::Cancelled`], so no path leaks a wait or a stale
    /// prompt. A wait cancelled out of the registry resolves here as the
    /// broker's failure policy: a [`ToolError`] of kind `Backend` the run
    /// raises at the Lua call site.
    async fn wait(&self) -> Result<String, ToolError> {
        let (token, receiver) = self.registry.create();
        let mut guard = WaitGuard {
            registry: Arc::clone(&self.registry),
            frames: self.frames.clone(),
            token,
            armed: true,
        };
        // No receiver means no socket is attached right now. Not a
        // failure: the registry retains the wait and the conversation
        // resends it on reconnect, so the lost push is repaired.
        let _ = guard.frames.send(WaitFrame::Required {
            token: guard.token.clone(),
        });
        match receiver.await {
            Ok(text) => {
                guard.armed = false;
                Ok(text)
            }
            // The sender died without a value: the wait was cancelled out
            // of the registry. The still-armed guard pushes the cancelled
            // frame on scope exit, so this path clears the client's prompt
            // too.
            Err(_) => Err(ToolError::message("the user-input wait was cancelled")
                .with_kind(ToolErrorKind::Backend)),
        }
    }
}
