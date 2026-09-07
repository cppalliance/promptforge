//! Chat-capable catalog filtering and generation tracking.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::watch;

/// One immutable chat-capable catalog generation.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChatCatalog {
    /// Monotonically increasing whenever the chat-capable subset changes.
    pub(crate) generation: u64,
    /// The chat-capable entries for this generation.
    pub(crate) models: Vec<serde_json::Value>,
}

/// Shared retained chat catalog and its generation notification.
#[derive(Debug, Clone)]
pub(super) struct ChatCatalogBus {
    latest: Arc<Mutex<ChatCatalog>>,
    generation: watch::Sender<u64>,
}

impl ChatCatalogBus {
    /// Creates an empty generation tracker.
    pub(super) fn new() -> Self {
        Self {
            latest: Arc::new(Mutex::new(ChatCatalog::default())),
            generation: watch::channel(0).0,
        }
    }

    /// Returns the current generation only when it can serve chat.
    pub(super) fn latest(&self) -> Option<ChatCatalog> {
        let chat = self
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        (!chat.models.is_empty()).then_some(chat)
    }

    /// Subscribes to changes of the chat-capable subset.
    pub(super) fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    /// Replaces the source snapshot, advancing only when its chat subset
    /// changes. Transcription-only churn does not disturb chat runs.
    pub(super) fn publish(&self, models: &[serde_json::Value]) {
        let models: Vec<serde_json::Value> = models
            .iter()
            .filter(|model| is_chat_capable(model))
            .cloned()
            .collect();
        let changed = {
            let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
            if latest.models == models {
                None
            } else {
                latest.generation = latest.generation.wrapping_add(1);
                latest.models = models;
                Some(latest.generation)
            }
        };
        if let Some(generation) = changed {
            self.generation.send_replace(generation);
        }
    }
}

/// Whether one gateway catalog row can back a chat model binding.
pub(crate) fn is_chat_capable(model: &serde_json::Value) -> bool {
    let has_id = model
        .get("id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.is_empty());
    let chat_kind = match model.get("kind") {
        None => true,
        Some(serde_json::Value::String(kind)) => kind == "chat",
        Some(_) => false,
    };
    has_id && chat_kind
}
