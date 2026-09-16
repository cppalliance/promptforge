//! The workspace subsystem's registration: its `/workspace/*` routes,
//! merged into the shell's API router, the workspace itself as its
//! state handle set, and its granted-roots view, which same-tier
//! subsystems read instead of naming this crate.

use std::sync::Arc;

use workshop_registry::{
    Registration, Registry, RouteRegistrarAdapter, WorkspaceRoots, WorkspaceRootsAdapter,
};

use crate::handlers;
use crate::workspace::Workspace;

/// Registers the workspace subsystem into the registry: its
/// `/workspace/*` routes (the confined filesystem and the
/// `/workspace/file/*` document routes), merged into the shell's API
/// router, the workspace itself as its state handle set, and its granted-roots
/// view, which same-tier subsystems read instead of naming this crate.
/// The returned guards keep the registrations alive; the composition
/// root holds them for the process lifetime.
pub fn register(
    registry: &Registry,
    workspace: &Workspace,
) -> (Registration, Registration, Registration) {
    let routes = registry.register_routes(Arc::new(RouteRegistrarAdapter::new({
        let workspace = workspace.clone();
        move || handlers::routes(workspace.clone())
    })));
    let state = registry.register_state::<Workspace>(Arc::new(workspace.clone()));
    let roots =
        registry.register_state::<dyn WorkspaceRoots>(Arc::new(WorkspaceRootsAdapter::new({
            let workspace = workspace.clone();
            move || workspace.granted_roots()
        })));
    (routes, state, roots)
}
