//! Typed catalog-event collection for one agent supervisor.

use workshop_menu::{CatalogBus, ChatCatalog};

use super::transition::{CatalogDisposition, SupervisorEvent};

/// One collected event and the catalog snapshot that produced it.
pub(super) struct CatalogEvent {
    pub(super) event: SupervisorEvent,
    pub(super) snapshot: Option<ChatCatalog>,
}

/// Collects the receiver's current catalog generation without waiting.
pub(super) fn current_catalog_event(
    catalog: &CatalogBus,
    generation: &mut tokio::sync::watch::Receiver<u64>,
) -> CatalogEvent {
    let observed = *generation.borrow_and_update();
    classify(catalog.latest_chat(), observed, None)
}

/// Waits for and classifies the next catalog generation.
pub(super) async fn next_catalog_event(
    catalog: &CatalogBus,
    generation: &mut tokio::sync::watch::Receiver<u64>,
    active_models: Option<&[serde_json::Value]>,
) -> CatalogEvent {
    if generation.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
    let observed = *generation.borrow_and_update();
    classify(catalog.latest_chat(), observed, active_models)
}

/// Classifies one retained snapshot against the run's frozen bindings.
fn classify(
    snapshot: Option<ChatCatalog>,
    observed_generation: u64,
    active_models: Option<&[serde_json::Value]>,
) -> CatalogEvent {
    let generation = snapshot
        .as_ref()
        .map_or(observed_generation, |chat| chat.generation);
    let disposition = match (&snapshot, active_models) {
        (None, _) => CatalogDisposition::Unavailable,
        (Some(chat), Some(active)) if chat.models != active => CatalogDisposition::Replacement,
        (Some(_), _) => CatalogDisposition::Retained,
    };
    CatalogEvent {
        event: SupervisorEvent::CatalogGeneration {
            generation,
            disposition,
        },
        snapshot,
    }
}
