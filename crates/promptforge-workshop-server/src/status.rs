//! The observer: a broadcast bus carrying status bar updates from every
//! subsystem to every connected `/ws` session.
//!
//! Anything with user-visible latency - startup phases, gateway round
//! trips, dictation and transcription, model downloads - reports what
//! it is doing as a [`StatusBarUpdate`]. The bus is a tokio broadcast
//! channel: updates fan out to all current subscribers, a send with no
//! subscribers is a no-op, and a subscriber that falls more than
//! [`STATUS_CHANNEL_CAPACITY`] updates behind is told it lagged and resumes
//! at the oldest retained update. Sending never blocks, so instrumenting a
//! hot path cannot stall the subsystem it observes.
//!
//! On the wire each update rides the workshop socket as an unsolicited
//! `{"type":"status",...}` frame (see [`StatusBarUpdate::frame`]). The
//! bus also retains the newest update, so a session that connects later
//! sends the current status immediately - the delivery contract's
//! resend-on-reconnect for ephemeral frames.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::broadcast;

use crate::protocol::{Activity, Progress, Severity, StatusBarUpdate};

/// Ring capacity of the status bus. Covers a startup burst plus an agent
/// turn's phase transitions with headroom; a receiver lagging past it
/// skips ahead rather than slowing the senders.
const STATUS_CHANNEL_CAPACITY: usize = 64;

/// The shared status bus: a cloneable handle onto the broadcast channel.
///
/// Clones are cheap (two `Arc` bumps) and all of them send into the same
/// channel, so subsystems take their own copy rather than a reference.
#[derive(Debug, Clone)]
pub struct StatusBus {
    sender: broadcast::Sender<StatusBarUpdate>,
    latest: Arc<Mutex<Option<StatusBarUpdate>>>,
}

impl StatusBus {
    /// Creates a bus with no subscribers, an empty ring, and no snapshot.
    pub(crate) fn new() -> Self {
        Self {
            sender: broadcast::channel(STATUS_CHANNEL_CAPACITY).0,
            latest: Arc::new(Mutex::new(None)),
        }
    }

    /// Subscribes to every update sent from this call onward.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<StatusBarUpdate> {
        self.sender.subscribe()
    }

    /// The most recently emitted update, retained so a session connecting
    /// later can send the current status as its snapshot.
    pub(crate) fn latest(&self) -> Option<StatusBarUpdate> {
        // A lock poisoned by a panicking peer recovers the value rather
        // than wedging the process (the crate's zone-two error policy).
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Broadcasts one update. With no subscribers this is a no-op; a slow
    /// subscriber skips ahead rather than applying backpressure.
    pub fn emit(&self, update: StatusBarUpdate) {
        // The retained copy (a second owner, hence the clone) is written
        // before the send, so a session that subscribes after the send
        // still finds this update as its snapshot.
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(update.clone());
        // A send only fails when there are no receivers, which is the bus's
        // resting state before the first client connects.
        let _ = self.sender.send(update);
    }

    /// Broadcasts one progress-free update at the given severity.
    pub(crate) fn report(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        severity: Severity,
        activity: Activity,
    ) {
        self.emit(StatusBarUpdate {
            label: label.into(),
            description: description.into(),
            progress: None,
            severity,
            activity,
        });
    }

    /// Broadcasts a user-visible status text.
    pub(crate) fn info(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.report(label, description, Severity::Info, activity);
    }

    /// Broadcasts a user-visible update carrying determinate progress,
    /// which the status bar renders as its progress bar.
    pub(crate) fn progress(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        progress: Progress,
        activity: Activity,
    ) {
        self.emit(StatusBarUpdate {
            label: label.into(),
            description: description.into(),
            progress: Some(progress),
            severity: Severity::Info,
            activity,
        });
    }

    /// Broadcasts an internal instrumentation pulse the UI does not
    /// display.
    pub(crate) fn debug(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.report(label, description, Severity::Debug, activity);
    }

    /// Broadcasts a failure the user should see.
    pub(crate) fn error(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.report(label, description, Severity::Error, activity);
    }

    /// Returns the bar to its resting state.
    pub(crate) fn idle(&self) {
        self.info("Ready", "idle", Activity::General);
    }
}

impl Default for StatusBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emitting_with_no_subscribers_is_a_no_op() {
        let bus = StatusBus::new();
        bus.info("Ready", "idle", Activity::General);
    }

    #[test]
    fn the_newest_update_is_retained_for_the_connect_snapshot() {
        let bus = StatusBus::new();
        assert!(bus.latest().is_none(), "an untouched bus has no snapshot");
        bus.info("one", "", Activity::General);
        bus.info("two", "", Activity::General);
        let latest = bus.latest().expect("the bus retains the newest update");
        assert_eq!(
            latest.label, "two",
            "a session connecting now snapshots the newest update"
        );
    }

    #[tokio::test]
    async fn a_lagged_receiver_skips_ahead_instead_of_blocking() {
        let bus = StatusBus::new();
        let mut receiver = bus.subscribe();
        let sent = STATUS_CHANNEL_CAPACITY + 10;
        for index in 0..sent {
            // Sends never block, however far behind the receiver is.
            bus.debug(format!("update {index}"), "", Activity::General);
        }
        let lag = match receiver.recv().await {
            Err(broadcast::error::RecvError::Lagged(skipped)) => skipped,
            Ok(got) => panic!("expected a lag report, got {got:?}"),
            Err(broadcast::error::RecvError::Closed) => panic!("the bus is still open"),
        };
        assert_eq!(lag, 10, "the ring retained only its capacity");
        let resumed = receiver.recv().await.expect("the ring still holds updates");
        assert_eq!(
            resumed.label, "update 10",
            "receiving resumes at the oldest retained update"
        );
    }
}
