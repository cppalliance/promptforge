//! The model catalog push channel: the gateway's catalog, rebroadcast to
//! every connected `/ws` session as a `{"type":"models",...}` frame.
//!
//! The heartbeat republishes the catalog when the gateway comes back
//! (unreachable to connected), so a UI that booted while the gateway was
//! down refreshes its model picker without a reload. Like the status bus,
//! the channel is a tokio broadcast: publishing never blocks, a publish
//! with no sessions is a no-op, and a lagging session skips ahead - every
//! push is a complete snapshot, so an overwritten one loses nothing.

use serde::Serialize;
use tokio::sync::broadcast;

/// Ring capacity of the catalog bus. Pushes are rare (one per gateway
/// reconnect) and each is a full snapshot, so a handful of slots is
/// generous.
const CATALOG_CHANNEL_CAPACITY: usize = 4;

/// One pushed model catalog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogPush {
    /// The gateway's `/v1/models` `data` array, verbatim.
    pub(crate) models: Vec<serde_json::Value>,
}

impl CatalogPush {
    /// The push as a wire frame: `"type": "models"` beside the array.
    pub(crate) fn frame(&self) -> CatalogFrame<'_> {
        CatalogFrame {
            kind: "models",
            models: &self.models,
        }
    }
}

/// The serialized shape of a catalog push on the socket, matching the chat
/// protocol's frame taxonomy.
#[derive(Debug, Serialize)]
pub(crate) struct CatalogFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    models: &'a [serde_json::Value],
}

/// The shared catalog bus: a cloneable handle onto the broadcast channel,
/// mirroring [`crate::status::StatusBus`].
#[derive(Debug, Clone)]
pub(crate) struct CatalogBus {
    sender: broadcast::Sender<CatalogPush>,
}

impl CatalogBus {
    /// Creates a bus with no subscribers and an empty ring.
    pub(crate) fn new() -> Self {
        Self {
            sender: broadcast::channel(CATALOG_CHANNEL_CAPACITY).0,
        }
    }

    /// Subscribes to every push sent from this call onward.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<CatalogPush> {
        self.sender.subscribe()
    }

    /// Broadcasts one catalog. With no subscribers this is a no-op; a slow
    /// subscriber skips ahead rather than applying backpressure.
    pub(crate) fn publish(&self, models: Vec<serde_json::Value>) {
        // A send only fails when there are no receivers, which is the bus's
        // resting state before the first client connects.
        let _ = self.sender.send(CatalogPush { models });
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

    #[test]
    fn a_catalog_push_serializes_as_a_models_frame() {
        let push = CatalogPush {
            models: vec![serde_json::json!({"id": "test-model", "object": "model"})],
        };
        let frame = serde_json::to_value(push.frame()).expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "models",
                "models": [{"id": "test-model", "object": "model"}],
            }),
            "the wire shape matches the chat protocol's frame taxonomy"
        );
    }

    #[tokio::test]
    async fn publishing_with_no_subscribers_is_a_no_op() {
        let bus = CatalogBus::new();
        bus.publish(vec![serde_json::json!({"id": "test-model"})]);
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
