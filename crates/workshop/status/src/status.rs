//! The status bus: a broadcast bus for status bar updates from every
//! subsystem to every connected `/ws` session.
//!
//! Anything with user-visible latency - startup phases, gateway round
//! trips, dictation and transcription, model downloads - reports what
//! it is doing as a [`StatusBarUpdate`]. The bus is a [`RetainedBus`]:
//! updates fan out to all current subscribers, a send with no
//! subscribers is a no-op, and a subscriber that falls more than
//! `STATUS_CHANNEL_CAPACITY` updates behind is told it lagged and resumes
//! at the oldest retained update. Sending never blocks, so instrumenting a
//! hot path cannot stall the subsystem it observes.
//!
//! On the wire each update is sent on the workshop socket as an unsolicited
//! `{"type":"status",...}` frame (see [`StatusBarUpdate::frame`]). The
//! bus also retains the newest update, so a session that connects later
//! sends the current status immediately - the delivery contract's
//! resend-on-reconnect for ephemeral frames.

use tokio::sync::broadcast;

use workshop_protocol::StatusBarUpdate;
use workshop_support::RetainedBus;

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
    bus: RetainedBus<StatusBarUpdate>,
}

impl StatusBus {
    /// Creates a bus with no subscribers, an empty ring, and no snapshot.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bus: RetainedBus::new(STATUS_CHANNEL_CAPACITY),
        }
    }

    /// Subscribes to every update sent from this call onward.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<StatusBarUpdate> {
        self.bus.subscribe()
    }

    /// The most recently emitted update, retained so a session connecting
    /// later can send the current status as its snapshot.
    #[must_use]
    pub fn latest(&self) -> Option<StatusBarUpdate> {
        self.bus.latest()
    }

    /// Broadcasts one update. With no subscribers this is a no-op; a slow
    /// subscriber skips ahead rather than applying backpressure.
    pub fn emit(&self, update: StatusBarUpdate) {
        self.bus.send(update);
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
    use workshop_protocol::{Activity, Severity};

    /// A non-busy, info-severity update: the tests read the label back,
    /// so the severity and activity are fixed.
    fn update(label: &str, description: &str) -> StatusBarUpdate {
        StatusBarUpdate {
            label: label.to_owned(),
            description: description.to_owned(),
            busy: false,
            severity: Severity::Info,
            activity: Activity::General,
        }
    }

    #[tokio::test]
    async fn emitting_with_no_subscribers_is_a_no_op() {
        let bus = StatusBus::new();
        bus.emit(update("Ready", "idle"));
        let mut late = bus.subscribe();
        assert!(
            matches!(late.try_recv(), Err(broadcast::error::TryRecvError::Empty)),
            "an emit with no subscribers queues nothing for a later one"
        );
    }

    #[test]
    fn the_newest_update_is_retained_for_the_connect_snapshot() {
        let bus = StatusBus::new();
        assert!(bus.latest().is_none(), "an untouched bus has no snapshot");
        bus.emit(update("one", ""));
        bus.emit(update("two", ""));
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
            bus.emit(update(&format!("update {index}"), ""));
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
