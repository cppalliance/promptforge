//! Shared handler state and the composition root that assembles the
//! per-feature routers into the workshop server.
//!
//! [`AppState`] delegates subsystem state to the [`Registry`]: each
//! extracted subsystem owns its state behind a narrow handle registered
//! there, and consumers fetch the handles through the registry's
//! type-keyed state collection. The harness every agent session runs in
//! is registered the same way. What remains here is the server's own
//! runtime infrastructure - the shared reconnect backoff - plus the
//! registration guards keeping every self-registration alive.

mod compose;
#[cfg(any(test, feature = "test-fixtures"))]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::Arc;

use axum::Router;

use workshop_gateway::GatewayHandles;
use workshop_gateway::gateway::GatewayError;
#[cfg(test)]
use workshop_gateway::gateway_binding::GatewayBinding;
use workshop_gateway::gateway_binding::{GatewaySnapshot, GatewayUpdater};
use workshop_gateway::heartbeat::GatewayHealth;
use workshop_gateway::resolve::ResolvedGateway;
use workshop_menu::MenuHandles;
use workshop_menu::catalog::CatalogBus;
use workshop_menu::menu::MenuBus;
use workshop_registry::{Push, Registration, Registry};
use workshop_status::StatusBus;
use workshop_support::{Config, DEFAULT_DEADLINE, ReconnectBackoff, with_deadline};
use workshop_workspace::Workspace;

use crate::agents::AgentSessions;
use crate::routes;

/// Address the server binds to when no override is given.
pub use workshop_support::DEFAULT_ADDR;

/// Shared handler state: the subsystem registry, the server's runtime
/// infrastructure, and the registration guards. Subsystem handles - the
/// gateway binding and health flag, the status, catalog, and menu buses,
/// the agent-session registry - are fetched through the registry's state
/// collection, where each subsystem self-registers them.
#[derive(Debug, Clone)]
pub struct AppState {
    backoff: ReconnectBackoff,
    registry: Registry,
    // Keeps the subsystems' self-registrations alive; dropping the last
    // state clone deregisters them.
    _registrations: Registrations,
}

/// The registration guards keeping every subsystem's self-registrations
/// alive: the push channels, the producer sinks, the state handles, the
/// route registrars, the background tasks, and the workspace roots view.
#[derive(Clone)]
struct Registrations {
    guards: Vec<Arc<Registration>>,
}

impl Registrations {
    fn new() -> Self {
        Self { guards: Vec::new() }
    }

    /// Holds one registration guard for the state's lifetime.
    fn hold(&mut self, guard: Registration) {
        self.guards.push(Arc::new(guard));
    }
}

impl fmt::Debug for Registrations {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Registrations")
            .field("held", &self.guards.len())
            .finish()
    }
}

impl AppState {
    /// Builds shared state from the loaded configuration, resolving the
    /// gateway endpoint first: a live gateway discovery file in the run
    /// directory wins over explicit `[gateway]` config.
    ///
    /// # Errors
    /// Returns [`StateError::Resolution`] when no live gateway discovery file
    /// exists and the config has no explicit gateway, and
    /// [`StateError::Gateway`] if the HTTP client cannot be built.
    pub fn new(config: &Config) -> Result<Self, StateError> {
        let gateway =
            workshop_gateway::resolve::resolve(&config.gateway).map_err(StateError::Resolution)?;
        state_with_gateway(config, &gateway)
    }

    /// The status bus, which every `/ws` session subscribes to so it can
    /// forward updates; producers report through [`AppState::push`].
    ///
    /// # Panics
    /// Panics when the composition root never registered the bus - a bug
    /// boot already refuses: [`state_with_gateway`] requires every
    /// contribution before sharing state.
    #[must_use]
    pub fn status(&self) -> StatusBus {
        self.registry
            .require::<StatusBus>()
            .map_or_else(|error| panic!("{error}"), |handles| (*handles).clone())
    }

    /// The push facade over the status, catalog, and menu sinks, held
    /// by every subsystem that reports what happened.
    #[must_use]
    pub fn push(&self) -> Push {
        self.registry.push()
    }

    /// The gateway subsystem's registered handles.
    fn gateway_handles(&self) -> GatewayHandles {
        self.registry
            .require::<GatewayHandles>()
            .map_or_else(|error| panic!("{error}"), |handles| (*handles).clone())
    }

    /// The menu subsystem's registered handles.
    fn menu_handles(&self) -> MenuHandles {
        self.registry
            .require::<MenuHandles>()
            .map_or_else(|error| panic!("{error}"), |handles| (*handles).clone())
    }

    /// One atomic Gateway endpoint and credential generation.
    pub(crate) fn gateway_snapshot(&self) -> Arc<GatewaySnapshot> {
        self.gateway_handles().binding().snapshot()
    }

    /// A clone of the currently published Gateway HTTP client.
    #[must_use]
    pub fn gateway_client(&self) -> crate::GatewayClient {
        self.gateway_snapshot().client().clone()
    }

    /// The replaceable Gateway binding shared with long-lived tasks.
    #[cfg(test)]
    pub(crate) fn gateway_binding(&self) -> GatewayBinding {
        self.gateway_handles().binding().clone()
    }

    /// Restricted local-Gateway authority for an embedding host.
    pub(crate) fn gateway_updater(&self) -> GatewayUpdater {
        self.gateway_handles().binding().updater()
    }

    /// Permanently revokes host publication before application teardown.
    pub(crate) fn close_gateway_publication(&self) {
        self.gateway_handles()
            .binding()
            .updater()
            .close_publication();
    }

    /// Shared gateway reachability, published by the heartbeat; the
    /// gateway-dependent routes read it to short-circuit while the gateway
    /// is down.
    #[must_use]
    pub fn health(&self) -> GatewayHealth {
        self.gateway_handles().health().clone()
    }

    /// The shared reconnect backoff: the heartbeat draws probe delays
    /// from it while the gateway is down, and the agent sessions reset it
    /// on useful work - a completed model reply.
    #[must_use]
    pub fn backoff(&self) -> ReconnectBackoff {
        self.backoff.clone()
    }

    /// The catalog bus, which the heartbeat publishes the refreshed model
    /// catalog to on a gateway reconnect and every `/ws` session forwards
    /// from.
    #[must_use]
    pub fn catalog(&self) -> CatalogBus {
        self.menu_handles().catalog().clone()
    }

    /// The menu bus, whose workbench snapshots every `/ws` session
    /// forwards and whose mutators the session's menu events drive.
    #[must_use]
    pub fn menu(&self) -> MenuBus {
        self.menu_handles().menu().clone()
    }

    /// The subsystem registry: the contribution collections the
    /// subsystems self-register into, so consumers reach them by type
    /// instead of by name.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The agent-session opener: discovery, launch, and the running
    /// sessions behind the `/agents/ws` socket, every one of them run in
    /// the harness. Sessions outlive sockets, so an embedding host ends
    /// one through [`AgentSessions::close`].
    ///
    /// # Panics
    /// Panics when the composition root never registered the opener - a
    /// bug boot already refuses: [`state_with_gateway`] requires every
    /// contribution before sharing state.
    #[must_use]
    pub fn agents(&self) -> AgentSessions {
        self.registry
            .require::<AgentSessions>()
            .map_or_else(|error| panic!("{error}"), |handles| (*handles).clone())
    }

    /// Reopens the workspace file that was open when the server last
    /// ran, and returns whether one was reopened. Boot awaits this after
    /// composing state and before the listener serves, so the first
    /// request already sees the restored grants. It never fails: a
    /// missing or corrupt pointer, a vanished target, or a refused file
    /// logs and leaves the workspace ephemeral.
    ///
    /// # Panics
    /// Panics when the composition root never registered the workspace -
    /// a bug boot already refuses: [`state_with_gateway`] requires every
    /// contribution before sharing state.
    pub async fn reopen_last_workspace(&self) -> bool {
        let workspace = self
            .registry
            .require::<Workspace>()
            .map_or_else(|error| panic!("{error}"), |handles| (*handles).clone());
        workspace.reopen_last().await
    }
}

/// One subsystem's `register` call, named so a boot-composition test can
/// omit it and watch startup refuse to share state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Omit {
    /// `workshop_status::register`.
    Status,
    /// `workshop_menu::register`.
    Menu,
    /// `workshop_gateway::register`.
    Gateway,
    /// `workshop_workspace::register`.
    Workspace,
    /// `agents::register`: the sessions routes, the harness, and the
    /// agent-session opener.
    Sessions,
}

/// Builds shared state against an already-resolved gateway endpoint: the
/// construction phase a host holding its own endpoint enters directly,
/// skipping gateway discovery file resolution.
///
/// The composition root: every subsystem is constructed here, registers
/// itself into the registry, and is thereafter reached through the
/// registry's collections - routes included, which [`router`] merges
/// from the route vector.
///
/// # Errors
/// Returns [`StateError::Gateway`] if the HTTP client cannot be built,
/// and [`StateError::Composition`] when a required contribution is
/// absent after every subsystem has registered.
pub fn state_with_gateway(
    config: &Config,
    gateway: &ResolvedGateway,
) -> Result<AppState, StateError> {
    compose::compose(config, gateway, None, None)
}

/// [`state_with_gateway`] with the profile switch's sidecar restart bound
/// replaced: the seam for a test that must trip the bound without
/// waiting out the production value.
///
/// # Errors
/// Returns [`StateError::Gateway`] if the HTTP client cannot be built,
/// and [`StateError::Composition`] when a required contribution is
/// absent after every subsystem has registered.
#[cfg(feature = "test-fixtures")]
pub fn state_with_gateway_and_restart_bound(
    config: &Config,
    gateway: &ResolvedGateway,
    restart_bound: std::time::Duration,
) -> Result<AppState, StateError> {
    compose::compose(config, gateway, None, Some(restart_bound))
}

/// [`state_with_gateway`] with one subsystem's `register` call removed:
/// the boot-failure test's seam, proving a missing required contribution
/// fails boot with a typed error naming it.
///
/// # Errors
/// Returns [`StateError::Gateway`] if the HTTP client cannot be built,
/// and [`StateError::Composition`] naming the omitted subsystem's
/// contribution.
pub fn state_with_gateway_omitting(
    config: &Config,
    gateway: &ResolvedGateway,
    omit: Omit,
) -> Result<AppState, StateError> {
    compose::compose(config, gateway, Some(omit), None)
}

/// A shared-state construction failure: rich, init-only, and never sent
/// over the wire (the HTTP failure type is `crate::error::AppError`).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StateError {
    /// The gateway HTTP client could not be built.
    #[non_exhaustive]
    #[error("build gateway client")]
    Gateway(#[source] GatewayError),

    /// No gateway endpoint could be resolved: no live gateway discovery file and
    /// no explicit `[gateway]` config.
    #[non_exhaustive]
    #[error("resolve the gateway endpoint")]
    Resolution(#[source] workshop_gateway::resolve::ResolveError),

    /// A required subsystem contribution was never registered: the
    /// composition root itself is broken, so boot fails naming the
    /// absent type instead of panicking later at first use.
    #[non_exhaustive]
    #[error("compose the subsystem registry")]
    Composition(#[from] workshop_registry::MissingContribution),
}

/// Returns the workshop server router with every route mounted: the
/// server's own feature routers from `crate::routes`, plus the extracted
/// subsystems' routers merged from the registry's route vector in
/// registration order - an empty vector is a graceful no-op. The API
/// routes sit behind the
/// `crate::cross_site` guard; `/health` and the UI assets stay outside it
/// so the desktop app probe, heartbeat, and initial navigation keep working.
/// Every response includes the `crate::csp` policy: the desktop app's webview
/// loads the UI as an External origin, so the server sets the page's
/// Content-Security-Policy. Each subsystem applies its own deadline tier:
/// the default on the workspace routes, the relay tier on `/v1/models`,
/// none on the WebSocket upgrades.
pub fn router(state: AppState) -> Router {
    let registry = state.registry().clone();
    let mut api = Router::new()
        .merge(routes::realtime::routes(state.clone()))
        .merge(routes::gateway_config::routes(state))
        .merge(with_deadline(routes::prompts::routes(), DEFAULT_DEADLINE));
    // The subsystems' routes merge in registration order; an empty
    // vector is a graceful no-op.
    for registrar in registry.routes() {
        api = api.merge(registrar.routes());
    }
    let api = api.layer(axum::middleware::from_fn(crate::cross_site::guard));
    Router::new()
        .merge(with_deadline(routes::assets::routes(), DEFAULT_DEADLINE))
        .merge(with_deadline(routes::health::routes(), DEFAULT_DEADLINE))
        .merge(api)
        // The outermost layer on the server's own routes: every response
        // is stamped with the CSP, error envelopes included, so the desktop app's
        // External-origin webview runs under the policy no matter which
        // route answered.
        .layer(axum::middleware::from_fn(crate::csp::header))
}
