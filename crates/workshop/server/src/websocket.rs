//! What the server's two WebSocket endpoints share: the frame-sending
//! helpers, the cross-site upgrade refusal, and [`SocketState`], the
//! route state both the `/ws` workshop socket
//! ([`crate::workshop_socket`]) and the sessions subsystem's routes
//! ([`crate::agents`]) wrap. Neither of those modules imports the other;
//! both import this one.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use workshop_gateway::{GatewayHandles, GatewayHealth, GatewaySnapshot};
use workshop_protocol::{ErrorEnvelope, ErrorFrame};
use workshop_registry::{Push, Registry};

/// The route state every socket route draws from: the subsystem registry
/// its handles are read through, and the server's WebSocket origin
/// policy. Each handle is read through the registry's type-keyed state
/// collection at the point of use, an `Option` whose `None` degrades the
/// feature rather than failing the route.
///
/// The origin policy is injected by the server as a plain function: the
/// cross-site guard is the server's security boundary (its `cross_site`
/// module), and the socket routes apply it to every upgrade without
/// owning the policy.
#[derive(Debug, Clone)]
pub(crate) struct SocketState {
    registry: Registry,
    origin_allowed: fn(&HeaderMap) -> bool,
}

impl SocketState {
    /// Builds the state over the subsystem registry and the server's
    /// origin policy.
    #[must_use]
    pub(crate) fn new(registry: Registry, origin_allowed: fn(&HeaderMap) -> bool) -> Self {
        Self {
            registry,
            origin_allowed,
        }
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

    /// Shared gateway reachability, published by the heartbeat; `None`
    /// while the gateway subsystem has not registered reads as the
    /// flag's optimistic default.
    pub(crate) fn health(&self) -> Option<GatewayHealth> {
        self.registry
            .state::<GatewayHandles>()
            .map(|handles| handles.health().clone())
    }

    /// One atomic Gateway endpoint and credential generation, or `None`
    /// while the gateway subsystem has not registered.
    pub(crate) fn gateway_snapshot(&self) -> Option<Arc<GatewaySnapshot>> {
        self.registry
            .state::<GatewayHandles>()
            .map(|handles| handles.binding().snapshot())
    }

    /// The server's WebSocket origin policy, applied to every upgrade.
    pub(crate) fn origin_allowed(&self, headers: &HeaderMap) -> bool {
        (self.origin_allowed)(headers)
    }
}

/// The 403 refusal every WebSocket upgrade answers a foreign `Origin`
/// with: the same `cross_site` envelope the server's guard middleware
/// renders for plain HTTP requests.
pub(crate) fn cross_site_refusal() -> Response {
    let envelope = ErrorEnvelope::new("cross-site request refused", "cross_site");
    // Serializing the envelope cannot fail: two strings only.
    let body = serde_json::to_string(&envelope)
        .unwrap_or_else(|_| "cross-site request refused".to_string());
    (
        axum::http::StatusCode::FORBIDDEN,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

/// Sends one JSON text frame; a false return means the client is gone.
pub(crate) async fn send_frame<F: serde::Serialize>(socket: &mut WebSocket, frame: &F) -> bool {
    // Serializing the protocol frames cannot fail: strings, integers, and
    // JSON values only. A frame that somehow cannot serialize is skipped,
    // which is not a gone client.
    let Ok(text) = serde_json::to_string(frame) else {
        return true;
    };
    socket.send(Message::Text(text.into())).await.is_ok()
}

/// Sends one `error` frame with `message`, tagged with the request's
/// `id` when there is one, ignoring a dead client.
pub(crate) async fn send_error(
    socket: &mut WebSocket,
    id: Option<&serde_json::Value>,
    message: impl Into<String>,
) {
    let _ = send_frame(socket, &ErrorFrame::new(message.into(), id)).await;
}
