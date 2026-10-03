//! The sessions subsystem's shared route state and its registry
//! registration: the state every sessions route handler draws its
//! handles from, the router constructor, and the `register` entry point
//! the composition root calls.

use std::ops::Deref;
use std::sync::Arc;

use axum::Router;
use axum::http::HeaderMap;
use axum::routing::get;

use workshop_registry::{Registration, Registry, RouteRegistrarAdapter};
use workshop_support::{RELAY_DEADLINE, with_deadline};

use super::{AgentSessions, relay, socket};
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

/// The sessions subsystem's registration guards: its routes and the
/// agent-session launcher. Dropping them deregisters the subsystem.
#[derive(Debug)]
#[must_use = "dropping the registrations deregisters the subsystem"]
pub(crate) struct SessionsRegistrations {
    /// The `/v1/models` and `/agents/ws` route registrar.
    pub(crate) routes: Registration,
    /// The agent-session launcher as a state handle.
    pub(crate) agents: Registration,
}

/// Registers the sessions subsystem into the registry: its routes, merged
/// into the server's API router, and the agent-session launcher as a
/// state handle. The returned guards keep the registrations alive; the
/// composition root holds them for the process lifetime.
pub(crate) fn register(
    registry: &Registry,
    state: &SessionsState,
    agents: &AgentSessions,
) -> SessionsRegistrations {
    let routes = registry.register_routes(Arc::new(RouteRegistrarAdapter::new({
        let state = state.clone();
        move || routes(state.clone())
    })));
    let agents = registry.register_state::<AgentSessions>(Arc::new(agents.clone()));
    SessionsRegistrations { routes, agents }
}
