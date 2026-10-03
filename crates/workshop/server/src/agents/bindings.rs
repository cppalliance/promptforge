//! The bindings the server pushes through the Harness's public API as
//! data: the chat-capable model catalog and the Host snapshot a run's
//! `ui()` and model resolution read (the menu's selected model and the
//! workspace's granted roots).
//!
//! The Harness never resolves a gateway, reads a menu, or names a
//! workspace crate; it observes catalog generation changes through the
//! values pushed here. [`push_bindings`] reads every source through the
//! registry's collections and pushes both, the Host snapshot first, so
//! the catalog that triggers a relaunch never finds a stale selection
//! behind it. [`forward`] is the long-lived half: it wakes on the
//! catalog's chat-generation watch, the menu's snapshot bus, and the
//! workspace's grant-set generation watch, and pushes again.

use harness::{CatalogBinding, Harness, HostSnapshot};
use tokio::sync::{broadcast, watch};
use workshop_menu::{CatalogBus, MenuHandles};
use workshop_registry::{Registry, WorkspaceRoots};
use workshop_support::recv_or_pending;

/// Pushes the server's current Host snapshot and chat catalog into
/// `harness`, each read through `registry` at this moment. An
/// unregistered subsystem leaves its binding at whatever the Harness last
/// saw (the Host snapshot's absent parts read as `null`).
pub(crate) fn push_bindings(registry: &Registry, harness: &Harness) {
    harness.set_host(host_snapshot(registry));
    if let Some(menu) = registry.state::<MenuHandles>() {
        harness.set_catalog(catalog_binding(menu.catalog()));
    }
}

/// The Host snapshot: `selected_model` from the menu's retained workbench
/// state and the granted workspace roots from the registry's roots slot,
/// so this crate reads the workspace through the slot the workspace
/// subsystem registered, as the sessions did before the Harness.
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
/// empty list - which the Harness reads as no catalog to launch under.
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

/// Keeps the Harness's bindings current: pushes both again whenever the
/// chat-capable catalog changes generation, the menu publishes a
/// snapshot, or the workspace's granted roots change. Returns at once when
/// no Harness is registered; otherwise it reads the Harness once and runs
/// until every source has closed (the server's state is gone) or the
/// graceful-shutdown handle aborts it.
///
/// A fresh watch receiver treats the current value as seen, so a change
/// landing between the composition root's push and these subscriptions
/// would otherwise reach the Harness only on the next change: the first
/// push happens here, after every subscription is taken.
pub(crate) async fn forward(registry: Registry) {
    let Some(harness) = registry.state::<Harness>() else {
        return;
    };
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
        if catalog_rx.is_none() && menu_rx.is_none() && roots_rx.is_none() {
            return;
        }
        tokio::select! {
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
