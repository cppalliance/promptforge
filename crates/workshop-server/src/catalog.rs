//! The chat-capable model catalog push channel, rebroadcast to every
//! connected `/ws` session as a `{"type":"models",...}` frame.
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

use tokio::sync::{broadcast, watch};

use crate::protocol::CatalogPush;

mod chat;
use chat::ChatCatalogBus;
pub(crate) use chat::{ChatCatalog, is_chat_capable};

/// Ring capacity of the catalog bus. Pushes are rare (one per gateway
/// reconnect) and each is a full snapshot, so a handful of slots is
/// generous.
const CATALOG_CHANNEL_CAPACITY: usize = 4;

/// The shared catalog bus: a cloneable handle onto the broadcast channel,
/// mirroring [`crate::status::StatusBus`].
#[derive(Debug, Clone)]
pub struct CatalogBus {
    sender: broadcast::Sender<CatalogPush>,
    latest: Arc<Mutex<Option<CatalogPush>>>,
    chat: ChatCatalogBus,
}

impl CatalogBus {
    /// Creates a bus with no subscribers, an empty ring, and no snapshot.
    pub(crate) fn new() -> Self {
        Self {
            sender: broadcast::channel(CATALOG_CHANNEL_CAPACITY).0,
            latest: Arc::new(Mutex::new(None)),
            chat: ChatCatalogBus::new(),
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

    /// The current non-empty chat-capable catalog generation.
    pub(crate) fn latest_chat(&self) -> Option<ChatCatalog> {
        self.chat.latest()
    }

    /// Subscribes to chat-capable catalog generation changes.
    pub(crate) fn subscribe_chat_generation(&self) -> watch::Receiver<u64> {
        self.chat.subscribe()
    }

    /// Broadcasts one catalog. With no subscribers this is a no-op; a slow
    /// subscriber skips ahead rather than applying backpressure.
    pub fn publish(&self, models: Vec<serde_json::Value>) {
        let models = models.into_iter().filter(is_chat_capable).collect();
        let push = CatalogPush { models };
        self.chat.publish(&push.models);
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
mod tests;
