//! The model catalog push channel: the gateway's catalog, rebroadcast to
//! every connected `/ws` session as a `{"type":"models",...}` frame.
//!
//! The heartbeat republishes the catalog when the gateway comes back
//! (unreachable to connected), so a UI that booted while the gateway was
//! down refreshes its model picker without a reload. Like the status bus,
//! the channel is a tokio broadcast: publishing never blocks, a publish
//! with no sessions is a no-op, and a lagging session skips ahead - every
//! push is a complete snapshot, so an overwritten one loses nothing. The
//! bus also retains the newest push, so a session that connects later
//! sends the current catalog immediately - the delivery contract's
//! resend-on-reconnect for ephemeral frames.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::broadcast;

use crate::protocol::CatalogPush;

/// Ring capacity of the catalog bus. Pushes are rare (one per gateway
/// reconnect) and each is a full snapshot, so a handful of slots is
/// generous.
const CATALOG_CHANNEL_CAPACITY: usize = 4;

/// The shared catalog bus: a cloneable handle onto the broadcast channel,
/// mirroring [`crate::status::StatusBus`].
#[derive(Debug, Clone)]
pub(crate) struct CatalogBus {
    sender: broadcast::Sender<CatalogPush>,
    latest: Arc<Mutex<Option<CatalogPush>>>,
}

impl CatalogBus {
    /// Creates a bus with no subscribers, an empty ring, and no snapshot.
    pub(crate) fn new() -> Self {
        Self {
            sender: broadcast::channel(CATALOG_CHANNEL_CAPACITY).0,
            latest: Arc::new(Mutex::new(None)),
        }
    }

    /// Subscribes to every push sent from this call onward.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<CatalogPush> {
        self.sender.subscribe()
    }

    /// The most recently published catalog, retained so a session
    /// connecting later can send the current catalog as its snapshot.
    pub(crate) fn latest(&self) -> Option<CatalogPush> {
        // A lock poisoned by a panicking peer recovers the value rather
        // than wedging the process (the crate's zone-two error policy).
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Broadcasts one catalog. With no subscribers this is a no-op; a slow
    /// subscriber skips ahead rather than applying backpressure.
    pub(crate) fn publish(&self, models: Vec<serde_json::Value>) {
        let push = CatalogPush { models };
        // The retained copy (a second owner, hence the clone) is written
        // before the send, so a session that subscribes after the send
        // still finds this push as its snapshot.
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(push.clone());
        // A send only fails when there are no receivers, which is the bus's
        // resting state before the first client connects.
        let _ = self.sender.send(push);
    }
}

impl Default for CatalogBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publishing_with_no_subscribers_is_a_no_op() {
        let bus = CatalogBus::new();
        bus.publish(vec![serde_json::json!({"id": "test-model"})]);
    }

    #[test]
    fn the_newest_push_is_retained_for_the_connect_snapshot() {
        let bus = CatalogBus::new();
        assert!(bus.latest().is_none(), "an untouched bus has no snapshot");
        bus.publish(vec![serde_json::json!({"id": "old"})]);
        bus.publish(vec![serde_json::json!({"id": "new"})]);
        let latest = bus.latest().expect("the bus retains the newest push");
        assert_eq!(
            latest.models[0]["id"], "new",
            "a session connecting now snapshots the newest catalog"
        );
    }

    #[tokio::test]
    async fn a_lagged_receiver_skips_ahead_instead_of_blocking() {
        let bus = CatalogBus::new();
        let mut receiver = bus.subscribe();
        for index in 0..=CATALOG_CHANNEL_CAPACITY {
            bus.publish(vec![serde_json::json!({"id": format!("model-{index}")})]);
        }
        match receiver.recv().await {
            Err(broadcast::error::RecvError::Lagged(1)) => {}
            other => panic!("expected a lag report of one, got {other:?}"),
        }
        let resumed = receiver.recv().await.expect("the ring still holds pushes");
        assert_eq!(
            resumed.models[0]["id"], "model-1",
            "receiving resumes at the oldest retained push"
        );
    }
}
