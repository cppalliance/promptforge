//! The workspace subsystem's registration and any handle types. It
//! registers its `/workspace/*` routes, merged into the server's API
//! router, the workspace itself as its state handle set, its
//! granted-roots view and change signal, which the server's
//! agent-session bindings read through the registry, and the shutdown
//! lever that closes the workspace file inside the server's graceful
//! stop.

use std::sync::Arc;

use workshop_registry::{
    BackgroundTaskAdapter, Registration, Registry, RouteRegistrarAdapter, ShutdownHandle,
    WorkspaceRoots, WorkspaceRootsAdapter,
};

use crate::handlers;
use crate::workspace::Workspace;

/// The workspace subsystem's registration guards: its routes, its state
/// handle, and its granted-roots view. Dropping them deregisters the
/// subsystem.
#[derive(Debug)]
#[must_use = "dropping the registrations deregisters the subsystem"]
pub struct WorkspaceRegistrations {
    /// The `/workspace/*` route registrar.
    pub routes: Registration,
    /// The workspace itself as its state handle.
    pub state: Registration,
    /// The granted-roots view the server's agent-session bindings read.
    pub roots: Registration,
}

/// Registers the workspace subsystem into the registry: its
/// `/workspace/*` routes (the confined filesystem and the
/// `/workspace/file/*` document routes), merged into the server's API
/// router, the workspace itself as its state handle set, and its
/// granted-roots view and change signal, which the server's agent-session
/// bindings read through the registry. The returned guards keep the
/// registrations alive; the composition root holds them for the process
/// lifetime.
pub fn register(registry: &Registry, workspace: &Workspace) -> WorkspaceRegistrations {
    let routes = registry.register_routes(Arc::new(RouteRegistrarAdapter::new({
        let workspace = workspace.clone();
        move || handlers::routes(workspace.clone())
    })));
    let state = registry.register_state::<Workspace>(Arc::new(workspace.clone()));
    let roots =
        registry.register_state::<dyn WorkspaceRoots>(Arc::new(WorkspaceRootsAdapter::new(
            {
                let workspace = workspace.clone();
                move || workspace.granted_roots()
            },
            {
                let workspace = workspace.clone();
                move || workspace.subscribe_roots()
            },
        )));
    WorkspaceRegistrations {
        routes,
        state,
        roots,
    }
}

/// Registers the subsystem's one background task: the shutdown lever
/// that closes the workspace file. The workspace-file actor already
/// runs from the moment a file is opened, so the task's `spawn` spawns
/// nothing; the adapter exists only to hand the registry a
/// [`ShutdownHandle`] the server awaits inside its graceful-shutdown
/// closure, where [`Workspace::close_backing`] folds the WAL into the
/// file and removes the sidecar before the runtime tears down. The
/// server's grace window bounds the whole drain and is the close's only
/// timeout. The returned guard keeps the registration alive; the
/// composition root holds it for the process lifetime.
pub fn register_tasks(registry: &Registry, workspace: &Workspace) -> Registration {
    registry.register_task(Arc::new(BackgroundTaskAdapter::new({
        let workspace = workspace.clone();
        move || {
            let workspace = workspace.clone();
            ShutdownHandle::new(move || async move { workspace.close_backing().await })
        }
    })))
}
