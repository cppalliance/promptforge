//! Catalog-generation waits for one agent supervisor.

use std::sync::atomic::Ordering;

use crate::catalog::{CatalogBus, ChatCatalog};

use super::super::AgentSession;

/// Waits for the first non-empty chat catalog or session close.
pub(super) async fn wait_for_chat_catalog(
    session: &AgentSession,
    catalog: &CatalogBus,
    generation: &mut tokio::sync::watch::Receiver<u64>,
) -> Option<ChatCatalog> {
    loop {
        let closed = session.closed.notified();
        tokio::pin!(closed);
        if session.closing.load(Ordering::SeqCst) {
            return None;
        }
        if let Some(chat) = catalog.latest_chat() {
            return Some(chat);
        }
        tokio::select! {
            () = &mut closed => {}
            changed = generation.changed() => {
                if changed.is_err() {
                    return None;
                }
            }
        }
    }
}

/// Waits for a usable generation with bindings different from this run.
pub(super) async fn wait_for_replacement_catalog(
    catalog: &CatalogBus,
    generation: &mut tokio::sync::watch::Receiver<u64>,
    active_generation: u64,
    active_models: &[serde_json::Value],
) -> Option<ChatCatalog> {
    loop {
        if generation.changed().await.is_err() {
            return None;
        }
        if let Some(chat) = catalog.latest_chat()
            && chat.generation != active_generation
            && chat.models != active_models
        {
            return Some(chat);
        }
    }
}
