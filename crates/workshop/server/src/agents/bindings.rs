//! The bindings the server pushes through the harness's public API as
//! data: the gateway endpoint and bearer, the chat-capable model
//! catalog, and the host snapshot a run's `ui()` and model resolution
//! read (the menu's selected model and the workspace's granted roots).
//!
//! The harness never resolves a gateway, reads a menu, or names a
//! workspace crate; it observes generation changes through the values
//! pushed here. [`push_bindings`] reads every source through the
//! registry's collections and pushes all three, host first, so the
//! binding that triggers a relaunch never finds a stale selection behind
//! it. [`forward`] is the long-lived half: it wakes on the gateway
//! binding's replacement watch, the catalog's chat-generation watch, the
//! menu's snapshot bus, and the workspace's grant-set generation watch,
//! and pushes again.

use harness_api::{CatalogBinding, GatewayBinding, Harness, HostSnapshot};
use tokio::sync::{broadcast, watch};
use workshop_gateway::{GatewayHandles, GatewaySnapshot};
use workshop_menu::{CatalogBus, MenuHandles};
use workshop_registry::{Registry, WorkspaceRoots};
use workshop_support::recv_or_pending;

/// Pushes the server's current host snapshot, chat catalog, and gateway
/// binding into `harness`, each read through `registry` at this moment.
/// An unregistered subsystem leaves its binding at whatever the harness
/// last saw (the host snapshot's absent parts read as `null`).
pub(crate) fn push_bindings(registry: &Registry, harness: &Harness) {
    harness.set_host(host_snapshot(registry));
    if let Some(menu) = registry.state::<MenuHandles>() {
        harness.set_catalog(catalog_binding(menu.catalog()));
    }
    if let Some(gateway) = registry.state::<GatewayHandles>() {
        harness.set_gateway(gateway_binding(&gateway.binding().snapshot()));
    }
}

/// The host snapshot: `selected_model` from the menu's retained workbench
/// state and the granted workspace roots from the registry's roots slot,
/// so this crate reads the workspace through the slot the workspace
/// subsystem registered, as the sessions did before the harness.
fn host_snapshot(registry: &Registry) -> HostSnapshot {
    let selected_model = registry
        .state::<MenuHandles>()
        .and_then(|handles| handles.menu().latest())
        .and_then(|snapshot| snapshot.selected_model);
    let workspace_roots = registry
        .state::<dyn WorkspaceRoots>()
        .map_or_else(Vec::new, |roots| roots.granted_roots());
    HostSnapshot {
        selected_model,
        workspace_roots,
    }
}

/// The chat catalog binding: the retained chat-capable generation, or,
/// when no chat-capable model exists, the current generation with an
/// empty list - which the harness reads as no catalog to launch under.
fn catalog_binding(catalog: &CatalogBus) -> CatalogBinding {
    match catalog.latest_chat() {
        Some(chat) => CatalogBinding {
            generation: chat.generation,
            models: chat.models,
        },
        None => CatalogBinding {
            generation: *catalog.subscribe_chat_generation().borrow(),
            models: Vec::new(),
        },
    }
}

/// The gateway binding for one published generation: its base URL, its
/// bearer, and the generation the server assigned before publishing it.
fn gateway_binding(snapshot: &GatewaySnapshot) -> GatewayBinding {
    GatewayBinding {
        base_url: snapshot.base_url().to_owned(),
        key: snapshot.api_key().to_owned(),
        generation: snapshot.generation(),
    }
}

/// Keeps the harness's bindings current: pushes all three again whenever
/// the gateway binding is replaced, the chat-capable catalog changes
/// generation, the menu publishes a snapshot, or the workspace's granted
/// roots change. Returns at once when no harness is registered; otherwise
/// it reads the harness once and runs until every source has closed (the
/// server's state is gone) or the graceful-shutdown handle aborts it.
///
/// A fresh watch receiver treats the current value as seen, so a change
/// landing between the composition root's push and these subscriptions
/// would otherwise reach the harness only on the next change: the first
/// push happens here, after every subscription is taken.
pub(crate) async fn forward(registry: Registry) {
    let Some(harness) = registry.state::<Harness>() else {
        return;
    };
    let mut gateway_rx = registry
        .state::<GatewayHandles>()
        .map(|handles| handles.binding().subscribe());
    let (mut catalog_rx, mut menu_rx) =
        registry
            .state::<MenuHandles>()
            .map_or((None, None), |handles| {
                (
                    Some(handles.catalog().subscribe_chat_generation()),
                    Some(handles.menu().subscribe()),
                )
            });
    let mut roots_rx = registry
        .state::<dyn WorkspaceRoots>()
        .map(|roots| roots.subscribe());
    push_bindings(&registry, &harness);
    loop {
        if gateway_rx.is_none() && catalog_rx.is_none() && menu_rx.is_none() && roots_rx.is_none() {
            return;
        }
        tokio::select! {
            open = changed(&mut gateway_rx) => {
                if !open {
                    gateway_rx = None;
                    continue;
                }
            }
            open = changed(&mut catalog_rx) => {
                if !open {
                    catalog_rx = None;
                    continue;
                }
            }
            open = changed(&mut roots_rx) => {
                if !open {
                    roots_rx = None;
                    continue;
                }
            }
            received = recv_or_pending(&mut menu_rx) => match received {
                // A lagged receiver lost intermediate snapshots; the push
                // below reads the retained newest one, so nothing is stale.
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => {
                    menu_rx = None;
                    continue;
                }
            },
        }
        push_bindings(&registry, &harness);
    }
}

/// Waits for an optional watch to change: `true` on a change, `false`
/// once its sender is gone, and forever pending when absent.
async fn changed(watch: &mut Option<watch::Receiver<u64>>) -> bool {
    match watch {
        Some(watch) => watch.changed().await.is_ok(),
        None => std::future::pending().await,
    }
}
#[cfg(test)]
#[path = "bindings-tests.rs"]
mod tests;
