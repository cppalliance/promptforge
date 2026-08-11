//! The task that delivers what an [`McpObserver`](super::McpObserver) queues.
//!
//! The observer sits on the run's own synchronous path and can only `try_send`
//! a [`Frame`]; awaiting the peer belongs here, on a task of its own, so nothing
//! the run does blocks on the client reading its stream. The final flush is
//! bounded for the same reason: a caption nobody reads is not worth a reply
//! nobody receives.

use std::time::Duration;

use rmcp::RoleServer;
use rmcp::model::{ProgressNotificationParam, ProgressToken};
use rmcp::service::Peer;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;

use super::Frame;

/// How long the reply waits for the pump to drain before abandoning it.
///
/// Sending a notification resolves only once the transport has accepted it, so
/// an unbounded wait lets a peer that stalled its stream without closing it
/// hold the whole `tools/call` open. Progress is best-effort in both
/// directions: a caption nobody reads is not worth a reply nobody receives.
///
/// A quarter second is chosen against the two things it trades off. A healthy
/// stream, local or across a LAN, accepts a queued frame in single-digit
/// milliseconds, so this is generous enough that the last caption is not lost
/// to an ordinary hiccup. A stalled one costs the caller a quarter second on a
/// reply that already took minutes to earn, which no user perceives.
pub(crate) const FLUSH_GRACE: Duration = Duration::from_millis(250);

/// The task that delivers what an [`McpObserver`](super::McpObserver) queues.
///
/// Ends when the observer is dropped, which closes the queue, or when the peer
/// stops accepting notifications.
#[derive(Debug)]
#[must_use = "the frames a run queued are delivered by the pump, so finish it"]
pub(crate) struct ProgressPump {
    /// The forwarding task.
    task: JoinHandle<()>,
}

impl ProgressPump {
    /// Spawns the pump that forwards `frames` to `peer` under `token`.
    ///
    /// # Panics
    /// Panics when called outside a Tokio runtime, since the pump runs on a
    /// task and that task is spawned here.
    pub(crate) fn spawn(
        frames: Receiver<Frame>,
        peer: Peer<RoleServer>,
        token: ProgressToken,
    ) -> ProgressPump {
        ProgressPump {
            task: tokio::spawn(pump(frames, peer, token)),
        }
    }

    /// A pump over an already-spawned task, for a test that wants to drive one
    /// that never resolves.
    #[cfg(test)]
    pub(crate) fn from_task(task: JoinHandle<()>) -> ProgressPump {
        ProgressPump { task }
    }

    /// Waits a bounded grace period for every frame queued before the observer
    /// was dropped, then abandons what is left.
    ///
    /// Awaiting this before answering the call is what keeps the last frame
    /// from arriving after the result it described. The wait is bounded
    /// because a peer that has stopped reading its stream would otherwise hold
    /// the reply open for as long as it liked.
    pub(crate) async fn finish(mut self) {
        match tokio::time::timeout(FLUSH_GRACE, &mut self.task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::debug!(%error, "the progress pump did not finish cleanly");
            }
            Err(_elapsed) => {
                self.task.abort();
                // Await the aborted handle so the task is joined here rather than
                // left detached to finish unobserved: the abort is the request,
                // and this await is what makes it done before the reply goes out.
                let _aborted = (&mut self.task).await;
                tracing::debug!("abandoned the progress pump, which was still sending");
            }
        }
    }
}

/// Forwards frames to the client until the queue closes or the peer refuses
/// one.
async fn pump(mut frames: Receiver<Frame>, peer: Peer<RoleServer>, token: ProgressToken) {
    while let Some(frame) = frames.recv().await {
        let notification = ProgressNotificationParam::new(token.clone(), f64::from(frame.progress))
            .with_message(frame.message);
        if let Err(error) = peer.notify_progress(notification).await {
            tracing::debug!(%error, "stopped reporting progress");
            return;
        }
    }
}
