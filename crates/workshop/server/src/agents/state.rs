//! The sessions subsystem's shared route state and its registry
//! registration: the state every sessions route handler draws its
//! handles from, the router constructor, and the `register` entry point
//! the composition root calls.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::HeaderMap;
use axum::routing::get;

use harness_api::Harness;
use workshop_gateway::{GatewayHandles, GatewaySnapshot};
use workshop_menu::{CatalogBus, MenuBus, MenuHandles};
use workshop_registry::{
    BackgroundTaskAdapter, Push, Registration, Registry, RouteRegistrarAdapter, ShutdownHandle,
};
use workshop_support::{RELAY_DEADLINE, with_deadline};

use super::{AgentSessions, bindings, relay, session, socket};

/// The shared state of the sessions subsystem's routes: the subsystem
/// registry every handle is read through, and the server's WebSocket
/// origin policy. The agent-session opener, the gateway endpoint binding
/// and reachability flag, and the catalog and menu buses are read
/// through the registry's type-keyed state collection at the point of
/// use, each an `Option` whose `None` degrades the feature the way the
/// status channel's absence always has.
///
/// The origin policy is injected by the server as a plain function: the
/// cross-site guard is the server's security boundary (its `cross_site`
/// module), and the subsystem applies it to every upgrade without owning
/// the policy.
#[derive(Debug, Clone)]
pub(crate) struct SessionsState {
    registry: Registry,
    origin_allowed: fn(&HeaderMap) -> bool,
    restart_bound: Duration,
}

/// How long a profile switch waits for a relaunched sidecar gateway to
/// publish a replacement generation serving the selection before the
/// switch fails. Model downloads never run inside this window (the boot
/// load publishes its listener first), so it covers process exit, the
/// supervisor's relaunch, and the bind.
pub(crate) const DEFAULT_RESTART_BOUND: Duration = Duration::from_secs(90);

impl SessionsState {
    /// Builds the route state over the subsystem registry and the
    /// server's origin policy, with the default restart bound.
    #[must_use]
    pub(crate) fn new(registry: Registry, origin_allowed: fn(&HeaderMap) -> bool) -> Self {
        Self {
            registry,
            origin_allowed,
            restart_bound: DEFAULT_RESTART_BOUND,
        }
    }

    /// Replaces the bound a profile switch waits for a relaunched sidecar
    /// (see [`DEFAULT_RESTART_BOUND`]); a host embedding a slower
    /// supervisor, or a test that must trip the bound, sets it here.
    #[must_use]
    pub(crate) fn with_restart_bound(mut self, bound: Duration) -> Self {
        self.restart_bound = bound;
        self
    }

    /// The bound a profile switch waits for a relaunched sidecar.
    pub(crate) fn restart_bound(&self) -> Duration {
        self.restart_bound
    }

    /// The agent-session opener behind `/agents/ws`, or `None` while
    /// the sessions subsystem has not registered.
    pub(crate) fn agents(&self) -> Option<AgentSessions> {
        self.registry
            .state::<AgentSessions>()
            .map(|agents| (*agents).clone())
    }

    /// One atomic Gateway endpoint and credential generation, or `None`
    /// while the gateway subsystem has not registered.
    pub(crate) fn gateway_snapshot(&self) -> Option<Arc<GatewaySnapshot>> {
        self.registry
            .state::<GatewayHandles>()
            .map(|handles| handles.binding().snapshot())
    }

    /// Shared gateway reachability, published by the heartbeat; `None`
    /// while the gateway subsystem has not registered reads as the
    /// flag's optimistic default.
    pub(crate) fn health(&self) -> Option<workshop_gateway::GatewayHealth> {
        self.registry
            .state::<GatewayHandles>()
            .map(|handles| handles.health().clone())
    }

    /// The catalog bus every `/ws` session forwards from, or `None`
    /// while the menu subsystem has not registered.
    pub(crate) fn catalog(&self) -> Option<CatalogBus> {
        self.registry
            .state::<MenuHandles>()
            .map(|handles| handles.catalog().clone())
    }

    /// The menu bus every `/ws` session forwards and drives, or `None`
    /// while the menu subsystem has not registered.
    pub(crate) fn menu(&self) -> Option<MenuBus> {
        self.registry
            .state::<MenuHandles>()
            .map(|handles| handles.menu().clone())
    }

    /// The subsystem registry: the status push channel and the push
    /// facade are reached through its collections.
    pub(crate) fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The push facade over the status, catalog, and menu sinks.
    pub(crate) fn push(&self) -> Push {
        self.registry.push()
    }

    /// The server's WebSocket origin policy, applied to every upgrade.
    pub(crate) fn origin_allowed(&self, headers: &HeaderMap) -> bool {
        (self.origin_allowed)(headers)
    }
}

/// The sessions subsystem's routes: the `/v1/models` catalog relay on the
/// relay deadline, and the `/ws` and `/agents/ws` WebSocket upgrades,
/// which answer immediately and then outlive any deadline.
pub(crate) fn routes(state: SessionsState) -> Router {
    with_deadline(
        Router::new().route("/v1/models", get(relay::models)),
        RELAY_DEADLINE,
    )
    .route("/ws", get(session::upgrade))
    .route("/agents/ws", get(socket::upgrade))
    .with_state(state)
}

/// Registers the sessions subsystem into the registry: its routes, merged
/// into the server's API router, the harness every agent session runs in,
/// and the agent-session opener, both as state handles. The returned
/// guards keep the registrations alive; the composition root holds them
/// for the process lifetime.
pub(crate) fn register(
    registry: &Registry,
    state: &SessionsState,
    harness: Arc<Harness>,
    agents: &AgentSessions,
) -> (Registration, Registration, Registration) {
    let routes = registry.register_routes(Arc::new(RouteRegistrarAdapter::new({
        let state = state.clone();
        move || routes(state.clone())
    })));
    let harness = registry.register_state::<Harness>(harness);
    let agents = registry.register_state::<AgentSessions>(Arc::new(agents.clone()));
    (routes, harness, agents)
}

/// Registers the sessions subsystem's background task: the bindings
/// forwarder that pushes the server's gateway binding, chat catalog, and
/// host snapshot into the registered harness again on every replacement.
/// The task spawns when the server starts serving and stops inside the
/// graceful-shutdown signal. The returned guard keeps the registration
/// alive; the composition root holds it for the process lifetime.
pub(crate) fn register_tasks(registry: &Registry) -> Registration {
    registry.register_task(Arc::new(BackgroundTaskAdapter::new({
        let registry = registry.clone();
        move || {
            let forwarder = tokio::spawn(bindings::forward(registry.clone()));
            ShutdownHandle::new(move || async move {
                forwarder.abort();
                let _ = forwarder.await;
            })
        }
    })))
}
