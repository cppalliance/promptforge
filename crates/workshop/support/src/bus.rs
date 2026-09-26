//! The retained broadcast bus: the generic transport pattern behind the
//! workshop server's status, catalog, and menu buses.
//!
//! A [`RetainedBus`] is a tokio broadcast channel plus a retained copy of
//! the newest value: sends fan out to all current subscribers, a send
//! with no subscribers is a no-op, and a subscriber that falls more than
//! the ring capacity behind is told it lagged and resumes at the oldest
//! retained value. Sending never blocks, so instrumenting a hot path
//! cannot stall the subsystem it observes. The retained newest value is
//! the resend-on-reconnect snapshot: a session that connects later reads
//! it directly instead of waiting for the next publish.
//!
//! The status, catalog, and menu buses are thin wrappers over this one
//! type; each wrapper owns its ring capacity and its intent-named helper
//! methods. [`recv_or_pending`] is the consumer side's shared helper for
//! a subscription that may be absent.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::broadcast;

/// A broadcast bus retaining the newest value sent: a cloneable handle
/// onto the channel and the snapshot.
///
/// Clones are cheap (two `Arc` bumps) and all of them send into the same
/// channel, so subsystems take their own copy rather than a reference.
#[derive(Debug, Clone)]
pub struct RetainedBus<T> {
    sender: broadcast::Sender<T>,
    latest: Arc<Mutex<Option<T>>>,
}

impl<T: Clone> RetainedBus<T> {
    /// Creates a bus with the given ring capacity, no subscribers, and no
    /// snapshot. A receiver lagging past `capacity` skips ahead rather
    /// than slowing the senders.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            sender: broadcast::channel(capacity).0,
            latest: Arc::new(Mutex::new(None)),
        }
    }

    /// Subscribes to every value sent from this call onward.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<T> {
        self.sender.subscribe()
    }

    /// The most recently sent value, retained so a consumer connecting
    /// later can take the current state as its snapshot.
    #[must_use]
    pub fn latest(&self) -> Option<T> {
        // A lock poisoned by a panicking peer recovers the value rather
        // than wedging the process.
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Retains `value` as the newest snapshot, then broadcasts it. With
    /// no subscribers the broadcast is a no-op and only the snapshot
    /// moves; a slow subscriber skips ahead rather than applying
    /// backpressure.
    pub fn send(&self, value: T) {
        // The retained copy (a second owner, hence the clone) is written
        // before the send, so a consumer that subscribes after the send
        // still finds this value as its snapshot.
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(value.clone());
        // A send only fails when there are no receivers, which is the
        // bus's resting state before the first client connects.
        let _ = self.sender.send(value);
    }
}

/// Receives from an optional subscription, pending forever when absent,
/// so a `select!` branch for a detached or unregistered channel simply
/// never fires.
///
/// # Errors
/// Returns the receiver's own [`broadcast::error::RecvError`]: `Lagged`
/// when it fell behind the ring, `Closed` once every sender is gone.
pub async fn recv_or_pending<T: Clone>(
    receiver: &mut Option<broadcast::Receiver<T>>,
) -> Result<T, broadcast::error::RecvError> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sending_with_no_subscribers_is_a_no_op() {
        let bus = RetainedBus::new(4);
        bus.send("one".to_string());
    }

    #[test]
    fn the_newest_value_is_retained_for_the_connect_snapshot() {
        let bus = RetainedBus::new(4);
        assert!(bus.latest().is_none(), "an untouched bus has no snapshot");
        bus.send(1);
        bus.send(2);
        assert_eq!(
            bus.latest(),
            Some(2),
            "a consumer connecting now snapshots the newest value"
        );
    }

    #[test]
    fn the_snapshot_moves_before_the_send_so_a_late_subscriber_sees_it() {
        let bus = RetainedBus::new(4);
        bus.send(7);
        let _late = bus.subscribe();
        assert_eq!(
            bus.latest(),
            Some(7),
            "the retained copy is written even when the send has no receivers"
        );
    }

    #[tokio::test]
    async fn a_lagged_receiver_skips_ahead_instead_of_blocking() {
        let capacity = 4;
        let bus = RetainedBus::new(capacity);
        let mut receiver = bus.subscribe();
        for index in 0..capacity + 10 {
            // Sends never block, however far behind the receiver is.
            bus.send(index);
        }
        let lag = match receiver.recv().await {
            Err(broadcast::error::RecvError::Lagged(skipped)) => skipped,
            Ok(got) => panic!("expected a lag report, got {got:?}"),
            Err(broadcast::error::RecvError::Closed) => panic!("the bus is still open"),
        };
        assert_eq!(lag, 10, "the ring retained only its capacity");
        let resumed = receiver.recv().await.expect("the ring still holds values");
        assert_eq!(
            resumed, 10,
            "receiving resumes at the oldest retained value"
        );
    }

    #[tokio::test]
    async fn clones_send_into_one_channel_and_share_one_snapshot() {
        let bus = RetainedBus::new(4);
        let clone = bus.clone();
        let mut receiver = bus.subscribe();
        clone.send("through the clone".to_string());
        assert_eq!(
            receiver
                .recv()
                .await
                .expect("the clone's send reaches the bus"),
            "through the clone"
        );
        assert_eq!(
            bus.latest().as_deref(),
            Some("through the clone"),
            "the clone's send moves the shared snapshot"
        );
    }

    #[tokio::test]
    async fn a_present_subscription_receives_what_its_channel_sends() {
        let (sender, receiver) = broadcast::channel(4);
        let mut receiver = Some(receiver);
        sender.send(3).expect("the receiver is subscribed");
        assert_eq!(
            recv_or_pending(&mut receiver)
                .await
                .expect("the value is queued"),
            3
        );
    }

    #[test]
    fn an_absent_subscription_pends_forever() {
        let mut receiver: Option<broadcast::Receiver<u32>> = None;
        let mut wait = std::pin::pin!(recv_or_pending(&mut receiver));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            wait.as_mut().poll(&mut context).is_pending(),
            "a detached select! branch must never fire"
        );
    }
}
