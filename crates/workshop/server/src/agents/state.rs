//! The sessions subsystem's shared route state and its registry
//! registration: the state every sessions route handler draws its
//! handles from, the router constructor, and the `register` entry point
//! the composition root calls.

use std::ops::Deref;
use std::sync::Arc;

use axum::Router;
use axum::http::HeaderMap;
use axum::routing::get;

use harness::Harness;
use workshop_registry::{
    BackgroundTaskAdapter, Registration, Registry, RouteRegistrarAdapter, ShutdownHandle,
};
use workshop_support::{RELAY_DEADLINE, with_deadline};

use super::{AgentSessions, bindings, relay, socket};
use crate::websocket::SocketState;

/// The shared state of the sessions subsystem's routes: the socket state
/// every socket route shares (registry, origin policy, and the gateway
/// and push accessors, reached through `Deref`), plus the agent-session
/// opener, read through the registry's type-keyed state collection at
/// the point of use as an `Option` whose `None` degrades the feature.
#[derive(Debug, Clone)]
pub(crate) struct SessionsState {
    socket: SocketState,
}

impl SessionsState {
    /// Builds the route state over the subsystem registry and the
    /// server's origin policy.
    #[must_use]
    pub(crate) fn new(registry: Registry, origin_allowed: fn(&HeaderMap) -> bool) -> Self {
        Self {
            socket: SocketState::new(registry, origin_allowed),
        }
    }

    /// The agent-session opener behind `/agents/ws`, or `None` while
    /// the sessions subsystem has not registered.
    pub(crate) fn agents(&self) -> Option<AgentSessions> {
        self.registry()
            .state::<AgentSessions>()
            .map(|agents| (*agents).clone())
    }
}

impl Deref for SessionsState {
    type Target = SocketState;

    fn deref(&self) -> &SocketState {
        &self.socket
    }
}

/// The sessions subsystem's routes: the `/v1/models` catalog relay on the
/// relay deadline, and the `/agents/ws` WebSocket upgrade, which answers
/// immediately and then outlives any deadline.
pub(crate) fn routes(state: SessionsState) -> Router {
    with_deadline(
        Router::new().route("/v1/models", get(relay::models)),
        RELAY_DEADLINE,
    )
    .route("/agents/ws", get(socket::upgrade))
    .with_state(state)
}

/// The sessions subsystem's registration guards: its routes, the harness
/// every agent session runs in, and the agent-session opener. Dropping
/// them deregisters the subsystem.
#[derive(Debug)]
#[must_use = "dropping the registrations deregisters the subsystem"]
pub(crate) struct SessionsRegistrations {
    /// The `/v1/models` and `/agents/ws` route registrar.
    pub(crate) routes: Registration,
    /// The harness as a state handle.
    pub(crate) harness: Registration,
    /// The agent-session opener as a state handle.
    pub(crate) agents: Registration,
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
) -> SessionsRegistrations {
    let routes = registry.register_routes(Arc::new(RouteRegistrarAdapter::new({
        let state = state.clone();
        move || routes(state.clone())
    })));
    let harness = registry.register_state::<Harness>(harness);
    let agents = registry.register_state::<AgentSessions>(Arc::new(agents.clone()));
    SessionsRegistrations {
        routes,
        harness,
        agents,
    }
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
