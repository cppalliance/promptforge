//! The composition root: constructs every subsystem, registers it into
//! the shared [`Registry`], and assembles the shared [`AppState`]. Each
//! subsystem owns a `register` helper here so the composition reads as a
//! single list of subsystem registrations in registration order.

use std::sync::Arc;

use harness_api::Harness;

use workshop_gateway::GatewayHandles;
use workshop_gateway::gateway_binding::GatewayBinding;
use workshop_gateway::heartbeat::GatewayHealth;
use workshop_gateway::resolve::ResolvedGateway;
use workshop_menu::MenuHandles;
use workshop_menu::catalog::CatalogBus;
use workshop_menu::menu::MenuBus;
use workshop_registry::{Push, Registry, WorkspaceRoots};
use workshop_status::StatusBus;
use workshop_support::{Config, ReconnectBackoff};
use workshop_user_state::UserStateStore;
use workshop_workspace::Workspace;

use super::{AppState, Omit, Registrations, StateError};
use crate::agents::{self, AgentSessions, SessionsState};

/// The composition root behind [`super::state_with_gateway`]; `omit`
/// removes one subsystem's `register` call for the boot-failure test, and
/// `restart_bound` replaces the sessions subsystem's sidecar restart
/// bound when given.
pub(super) fn compose(
    config: &Config,
    gateway: &ResolvedGateway,
    omit: Option<Omit>,
    restart_bound: Option<std::time::Duration>,
) -> Result<AppState, StateError> {
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    // The per-profile model memory lives in the state directory; a bad
    // or missing memory file costs the memory, never startup.
    let state_dir = &config.server.state_dir;
    // A crash between an atomic write's temp file and its rename
    // orphans the temp; boot is the one moment the directory is
    // known and quiet, so it is swept here.
    workshop_support::sweep_orphaned_temps(state_dir);
    let menu = MenuBus::new(catalog.clone(), Some(state_dir));
    // The subsystems self-register: consumers reach their channels,
    // sinks, state handles, routes, and tasks through the registry's
    // collections instead of by name.
    let registry = Registry::new();
    let mut registrations = Registrations::new();
    register_status(&registry, &mut registrations, &status, omit);
    register_menu(&registry, &mut registrations, &catalog, &menu, omit);
    let push = registry.push();
    let backoff = register_gateway(&registry, &mut registrations, gateway, &push, omit)?;
    register_workspace(&registry, &mut registrations, state_dir, omit);
    register_user_state(&registry, &mut registrations, state_dir);
    register_sessions(
        &registry,
        &mut registrations,
        config,
        &backoff,
        restart_bound,
        omit,
    );
    // The boot contract: every subsystem's handle set is present before
    // state is shared, so a missing contribution fails here, naming the
    // type, instead of panicking later at first use.
    registry.require::<StatusBus>()?;
    registry.require::<MenuHandles>()?;
    registry.require::<GatewayHandles>()?;
    registry.require::<Harness>()?;
    registry.require::<AgentSessions>()?;
    registry.require::<Workspace>()?;
    registry.require::<dyn WorkspaceRoots>()?;
    registry.require::<UserStateStore>()?;
    push.push_idle();
    Ok(AppState {
        backoff,
        registry,
        _registrations: registrations,
    })
}

/// The status subsystem: its bus plus the push channel, sink, and state
/// handle it self-registers.
fn register_status(
    registry: &Registry,
    registrations: &mut Registrations,
    status: &StatusBus,
    omit: Option<Omit>,
) {
    if omit != Some(Omit::Status) {
        let (channel, sink, state) = workshop_status::register(registry, status);
        registrations.hold(channel);
        registrations.hold(sink);
        registrations.hold(state);
    }
}

/// The menu subsystem: its catalog and menu buses plus the sinks and
/// state handle it self-registers.
fn register_menu(
    registry: &Registry,
    registrations: &mut Registrations,
    catalog: &CatalogBus,
    menu: &MenuBus,
    omit: Option<Omit>,
) {
    if omit != Some(Omit::Menu) {
        let (catalog_sink, menu_sink, menu_state) =
            workshop_menu::register(registry, catalog, menu);
        registrations.hold(catalog_sink);
        registrations.hold(menu_sink);
        registrations.hold(menu_state);
    }
}

/// The gateway subsystem: reports the resolved gateway, builds the
/// replaceable binding, and self-registers its handles and background
/// tasks. Returns the shared reconnect backoff the sessions subsystem
/// later draws from.
fn register_gateway(
    registry: &Registry,
    registrations: &mut Registrations,
    gateway: &ResolvedGateway,
    push: &Push,
    omit: Option<Omit>,
) -> Result<ReconnectBackoff, StateError> {
    // Startup phases are reported as they run; with no client connected
    // yet these land on an empty bus, ready for the first session.
    workshop_gateway::resolve::report(gateway, push);
    let gateway_binding = GatewayBinding::new_with_identity(
        gateway.base_url(),
        gateway.api_key(),
        gateway.identity().cloned(),
    )
    .map_err(StateError::Gateway)?;
    let backoff = ReconnectBackoff::new();
    let health = GatewayHealth::new();
    let gateway_handles = GatewayHandles::new(gateway_binding, health.clone());
    if omit != Some(Omit::Gateway) {
        registrations.hold(workshop_gateway::register(
            registry,
            gateway_handles.clone(),
        ));
    }
    // The background tasks register beside the state handles; the server
    // spawns them from the registry's task vector when it starts
    // serving.
    let (heartbeat, subscriber) =
        workshop_gateway::register_tasks(registry, &gateway_handles, backoff.clone());
    registrations.hold(heartbeat);
    registrations.hold(subscriber);
    Ok(backoff)
}

/// The workspace subsystem: its file handle plus the routes, state,
/// roots, and shutdown task it self-registers.
fn register_workspace(
    registry: &Registry,
    registrations: &mut Registrations,
    state_dir: &std::path::Path,
    omit: Option<Omit>,
) {
    // The workspace remembers its last-used file in the state directory;
    // boot follows that memory through `reopen_last_workspace` once the
    // runtime is up, since the reopen is async and composition is not.
    let workspace = Workspace::with_state_dir(state_dir);
    if omit != Some(Omit::Workspace) {
        let (routes, state, roots) = workshop_workspace::register(registry, &workspace);
        registrations.hold(routes);
        registrations.hold(state);
        registrations.hold(roots);
        // The shutdown lever that closes the workspace file inside the
        // graceful stop, so a quit leaves one complete file and no
        // sidecar.
        registrations.hold(workshop_workspace::register_tasks(registry, &workspace));
    }
}

/// The user-state subsystem: the account-scoped UI state store plus the
/// routes and state handle it self-registers.
fn register_user_state(
    registry: &Registry,
    registrations: &mut Registrations,
    state_dir: &std::path::Path,
) {
    // The account-scoped UI state lives beside the menu memory in the
    // state directory; a bad or missing file costs the state, never
    // startup.
    let user_state = Arc::new(UserStateStore::new(state_dir));
    let (routes, state) = workshop_user_state::register(registry, user_state);
    registrations.hold(routes);
    registrations.hold(state);
}

/// The harness (agent-sessions) subsystem: the harness, the agent-session
/// opener, and the `/ws` sessions state, plus the routes and bindings
/// task it self-registers.
fn register_sessions(
    registry: &Registry,
    registrations: &mut Registrations,
    config: &Config,
    backoff: &ReconnectBackoff,
    restart_bound: Option<std::time::Duration>,
    omit: Option<Omit>,
) {
    // Agent sessions run in the harness, the engine's production host,
    // built here like every other subsystem and reached through the
    // registry; `agents` pushes the server's state through its public API.
    let harness = agents::harness_for(config, registry);
    let agents = AgentSessions::new(registry.clone(), backoff.clone());
    let mut sessions = SessionsState::new(registry.clone(), crate::cross_site::origin_allowed);
    if let Some(bound) = restart_bound {
        sessions = sessions.with_restart_bound(bound);
    }
    if omit != Some(Omit::Sessions) {
        let (routes, harness, agents) = agents::register(registry, &sessions, harness, &agents);
        registrations.hold(routes);
        registrations.hold(harness);
        registrations.hold(agents);
        // The bindings forwarder, spawned with serving like every task.
        registrations.hold(agents::register_tasks(registry));
    }
}
