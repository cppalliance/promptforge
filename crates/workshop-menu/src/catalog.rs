//! The chat-capable model catalog push channel, rebroadcast to every
//! connected `/ws` session as a `{"type":"models",...}` frame.
//!
//! The heartbeat republishes the catalog when the gateway comes back
//! (unreachable to connected), so a UI that booted while the gateway was
//! down refreshes its model picker without a reload. The channel is a
//! [`RetainedBus`]: publishing never blocks, a publish with no sessions
//! is a no-op, and a lagging session skips ahead - every push is a
//! complete snapshot, so an overwritten one loses nothing. The bus also
//! retains the newest push, so a session that connects later sends the
//! current catalog immediately - the delivery contract's
//! resend-on-reconnect for ephemeral frames.

use tokio::sync::{broadcast, watch};

use workshop_protocol::CatalogPush;
use workshop_support::RetainedBus;

#[path = "catalog-chat.rs"]
mod chat;
pub use chat::ChatCatalog;
use chat::ChatCatalogBus;
pub use workshop_protocol::is_chat_capable;

/// Ring capacity of the catalog bus. Pushes are rare (one per gateway
/// reconnect) and each is a full snapshot, so a handful of slots is
/// generous.
const CATALOG_CHANNEL_CAPACITY: usize = 4;

/// The shared catalog bus: a cloneable handle onto the broadcast channel,
/// mirroring the status subsystem's [`RetainedBus`]-backed status bus.
#[derive(Debug, Clone)]
pub struct CatalogBus {
    bus: RetainedBus<CatalogPush>,
    chat: ChatCatalogBus,
}

impl CatalogBus {
    /// Creates a bus with no subscribers, an empty ring, and no snapshot.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bus: RetainedBus::new(CATALOG_CHANNEL_CAPACITY),
            chat: ChatCatalogBus::new(),
        }
    }

    /// Subscribes to every push sent from this call onward.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<CatalogPush> {
        self.bus.subscribe()
    }

    /// The most recently published catalog, retained so a session
    /// connecting later can send the current catalog as its snapshot.
    #[must_use]
    pub fn latest(&self) -> Option<CatalogPush> {
        self.bus.latest()
    }

    /// The current non-empty chat-capable catalog generation.
    #[must_use]
    pub fn latest_chat(&self) -> Option<ChatCatalog> {
        self.chat.latest()
    }

    /// Subscribes to chat-capable catalog generation changes.
    #[must_use]
    pub fn subscribe_chat_generation(&self) -> watch::Receiver<u64> {
        self.chat.subscribe()
    }

    /// Broadcasts one catalog. With no subscribers this is a no-op; a slow
    /// subscriber skips ahead rather than applying backpressure.
    pub fn publish(&self, models: Vec<serde_json::Value>) {
        let models = models.into_iter().filter(is_chat_capable).collect();
        let push = CatalogPush { models };
        self.chat.publish(&push.models);
        self.bus.send(push);
    }
}

impl Default for CatalogBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "catalog-tests.rs"]
mod tests;
