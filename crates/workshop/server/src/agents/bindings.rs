//! The Host snapshot a conversation's run carries in its request: the
//! menu's selected model and the workspace's granted roots, read through
//! the registry as the run starts, which the run's `ui()` and model
//! resolution read for the whole run.

use harness::HostSnapshot;
use workshop_menu::MenuHandles;
use workshop_registry::{Registry, WorkspaceRoots};

/// The Host snapshot: `selected_model` from the menu's retained workbench
/// state and the granted workspace roots from the registry's roots slot,
/// so this crate reads the workspace through the slot the workspace
/// subsystem registered. An unregistered subsystem's part of the snapshot
/// reads as `null`.
pub(super) fn host_snapshot(registry: &Registry) -> HostSnapshot {
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

#[cfg(test)]
#[path = "bindings-tests.rs"]
mod tests;
