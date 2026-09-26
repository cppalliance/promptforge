//! The `/ws` WebSocket endpoint: one persistent socket for the
//! workshop's downstream JSON - unsolicited status updates from the
//! status bus, model catalog pushes, and workbench snapshots - plus the
//! inbound Model-menu events.
//!
//! A client upgrades `GET /ws` once. The Model-menu events arrive on
//! that socket. `{"type":"select_model","model":"..."}` selects the
//! chat model: the menu validates the id against the retained catalog and
//! publishes the fresh workbench snapshot, handled inline because a map
//! lookup and a broadcast send cost microseconds; an unknown model is
//! refused with an `error` frame. `{"type":"switch_profile","name":...}`
//! selects a gateway profile (`null` selects no profile): `begin_switch`
//! publishes the pending snapshot (`switching` set, `chat_ready` false)
//! before the frame handler returns, and the selection itself runs on
//! its own task - it persists the selection on the gateway, restarts a
//! supervised sidecar when the gateway must reload, reports each step as
//! determinate status-bar progress, refetches the profile state and model
//! catalog, and settles the menu. A second switch while one runs is
//! refused with an `error` frame. Both events echo an `id` on their
//! refusals when the frame included one. A frame that is not a
//! well-formed menu event is answered with an `error` frame and the
//! session continues. Chat itself is on the `/agents/ws` socket, served
//! by the sessions subsystem.
//!
//! One task owns the socket: a single `select!` loop reads inbound frames
//! and writes every outbound frame itself - no outbox channel, no writer
//! task. Status updates from the status subsystem, catalog pushes and
//! workbench snapshots from the menu subsystem flow as they publish. On
//! connect the session first sends the retained status, catalog, and
//! workbench snapshots, honoring the delivery contract's resend promise
//! (see `workshop-protocol`) - the UI boots from this socket alone, with
//! zero HTTP state fetches; after that the buses forward as they publish,
//! and a session too slow to drain them skips ahead to the newest
//! snapshot rather than slowing the producers.
//!
//! The status channel is reached through the subsystem registry
//! ([`SocketState::registry`]), not named directly: an unregistered
//! slot degrades the session to no status frames rather than failing it.

#[path = "workshop_socket-menu.rs"]
mod menu;

use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use tokio::sync::broadcast;

use workshop_menu::{CatalogBus, MenuBus, MenuHandles};
use workshop_registry::{Registration, Registry, RouteRegistrarAdapter};
use workshop_support::recv_or_pending;

use crate::websocket::{SocketState, cross_site_refusal, send_error, send_frame};

use self::menu::{select_model, start_switch};

/// How long a profile switch waits for a relaunched sidecar gateway to
/// publish a replacement generation serving the selection before the
/// switch fails. Model downloads never run inside this window (the boot
/// load publishes its listener first), so it covers process exit, the
/// supervisor's relaunch, and the bind.
pub(crate) const DEFAULT_RESTART_BOUND: Duration = Duration::from_secs(90);

/// The `/ws` route state: the socket state every socket route shares
/// (registry, origin policy, and the gateway and push accessors, reached
/// through `Deref`), plus the catalog and menu buses the session forwards
/// and the bound a profile switch waits for a relaunched sidecar.
#[derive(Debug, Clone)]
pub(crate) struct WorkshopSocketState {
    socket: SocketState,
    restart_bound: Duration,
}

impl WorkshopSocketState {
    /// Builds the route state over the subsystem registry and the
    /// server's origin policy, with the default restart bound.
    #[must_use]
    pub(crate) fn new(registry: Registry, origin_allowed: fn(&HeaderMap) -> bool) -> Self {
        Self {
            socket: SocketState::new(registry, origin_allowed),
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

    /// The catalog bus every `/ws` session forwards from, or `None`
    /// while the menu subsystem has not registered.
    pub(crate) fn catalog(&self) -> Option<CatalogBus> {
        self.registry()
            .state::<MenuHandles>()
            .map(|handles| handles.catalog().clone())
    }

    /// The menu bus every `/ws` session forwards and drives, or `None`
    /// while the menu subsystem has not registered.
    pub(crate) fn menu(&self) -> Option<MenuBus> {
        self.registry()
            .state::<MenuHandles>()
            .map(|handles| handles.menu().clone())
    }
}

impl Deref for WorkshopSocketState {
    type Target = SocketState;

    fn deref(&self) -> &SocketState {
        &self.socket
    }
}

/// The `/ws` route. The upgrade answers immediately and then outlives
/// any deadline, so none applies.
pub(crate) fn routes(state: WorkshopSocketState) -> Router {
    Router::new().route("/ws", get(upgrade)).with_state(state)
}

/// Registers the `/ws` route into the registry, merged into the server's
/// API router. The returned guard keeps the registration alive; the
/// composition root holds it for the process lifetime.
pub(crate) fn register(registry: &Registry, state: &WorkshopSocketState) -> Registration {
    registry.register_routes(Arc::new(RouteRegistrarAdapter::new({
        let state = state.clone();
        move || routes(state.clone())
    })))
}

/// Connection ids for log correlation, handed out in connection order.
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// Logs the connection close when the connection task ends, however it
/// ends, so the connection loop's exit paths need no cleanup calls.
struct ConnectionLog {
    connection: u64,
}

impl Drop for ConnectionLog {
    fn drop(&mut self) {
        tracing::info!(connection = self.connection, "workshop socket closed");
    }
}

/// Upgrades a `GET /ws` request to a WebSocket connection. A foreign
/// `Origin` is refused with 403: WS upgrades bypass Sec-Fetch in older
/// browsers, so the server's loopback origin policy guards the upgrade
/// itself.
async fn upgrade(
    State(state): State<WorkshopSocketState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.origin_allowed(&headers) {
        return cross_site_refusal();
    }
    ws.on_upgrade(move |socket| run_connection(socket, state))
}

/// Runs one connection until the socket closes or fails: a single
/// `select!` loop owning the socket for both reading and writing.
async fn run_connection(mut socket: WebSocket, state: WorkshopSocketState) {
    let connection = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
    tracing::info!(connection, "workshop socket opened");
    let _closed = ConnectionLog { connection };

    // Subscribe before snapshotting, so an update emitted between the two
    // arrives at least once; the possible duplicate is harmless because
    // status and catalog frames are complete snapshots. Every bus comes
    // from the registry's state collection: unregistered is a graceful
    // no-op, so the branch below pends forever instead of failing.
    let status = state
        .registry()
        .state::<dyn workshop_registry::StatusChannel>();
    let mut status_rx = status.as_ref().map(|channel| channel.subscribe());
    let mut catalog_rx = state.catalog().map(|catalog| catalog.subscribe());
    let mut menu_rx = state.menu().map(|menu| menu.subscribe());
    // The delivery contract resends the current status, catalog, and
    // workbench snapshots on reconnect; the buses retain the newest copy
    // for exactly this send, so the UI boots with zero HTTP state fetches.
    // The status line is the one exception: a retained heartbeat transition
    // ("Connected to gateway") describes a past moment, so the join line is
    // recomputed from the current probe instead of replayed stale.
    let retained = status.as_ref().and_then(|channel| channel.latest());
    let join = match state.health() {
        Some(health) => workshop_gateway::join_status(retained, &health),
        None => retained,
    };
    if let Some(update) = join
        && !send_frame(&mut socket, &update.frame()).await
    {
        return;
    }
    if let Some(catalog) = state.catalog().and_then(|catalog| catalog.latest())
        && !send_frame(&mut socket, &catalog.frame()).await
    {
        return;
    }
    if let Some(snapshot) = state.menu().and_then(|menu| menu.latest())
        && !send_frame(&mut socket, &snapshot.frame()).await
    {
        return;
    }

    // The buses close only when the server state tears down; a closed bus
    // drops its receiver, which leaves its branch pending rather than
    // spinning the loop on `Closed`. An unregistered bus has no receiver
    // from the start, so its branch never runs.
    loop {
        tokio::select! {
            // Biased, buses first: draining them ahead of inbound bounds
            // their staleness at one frame, and the client sends at human
            // pace, so inbound can never starve.
            biased;
            // The ephemeral path: bounded broadcasts. A lagged receiver
            // skips ahead to the retained window, which is a resync
            // because every status and catalog frame is a complete
            // snapshot.
            received = recv_or_pending(&mut status_rx) => match received {
                Ok(update) => {
                    if !send_frame(&mut socket, &update.frame()).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(connection, skipped, "status receiver lagged; skipped updates");
                }
                Err(broadcast::error::RecvError::Closed) => status_rx = None,
            },
            received = recv_or_pending(&mut catalog_rx) => match received {
                Ok(catalog) => {
                    if !send_frame(&mut socket, &catalog.frame()).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(connection, skipped, "catalog receiver lagged; skipped pushes");
                }
                Err(broadcast::error::RecvError::Closed) => catalog_rx = None,
            },
            received = recv_or_pending(&mut menu_rx) => match received {
                Ok(snapshot) => {
                    if !send_frame(&mut socket, &snapshot.frame()).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(connection, skipped, "menu receiver lagged; skipped snapshots");
                }
                Err(broadcast::error::RecvError::Closed) => menu_rx = None,
            },
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    handle_frame(&state, &text, &mut socket).await;
                }
                // Binary frames are ignored here; pings and pongs are
                // answered by axum itself.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_))) => {}
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    tracing::warn!(connection, %error, "workshop socket failed");
                    break;
                }
            },
        }
    }
}

/// Handles one inbound text frame: `select_model` and `switch_profile`
/// drive the Model menu, and anything else is answered with an `error`
/// frame. Refusals echo the frame's `id` when it included one.
async fn handle_frame(state: &WorkshopSocketState, text: &str, socket: &mut WebSocket) {
    let frame: serde_json::Value = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(error) => {
            send_error(socket, None, format!("invalid JSON frame: {error}")).await;
            return;
        }
    };
    // The event id, echoed on the refusal so the client can correlate it.
    // Absent and null both mean untagged.
    let id = frame.get("id").cloned().filter(|id| !id.is_null());
    let kind = frame.get("type").and_then(serde_json::Value::as_str);
    if kind == Some("select_model") {
        select_model(state, id.as_ref(), &frame, socket).await;
        return;
    }
    if kind == Some("switch_profile") {
        start_switch(state, id.as_ref(), &frame, socket).await;
        return;
    }
    send_error(
        socket,
        id.as_ref(),
        "unknown frame type; expected \"select_model\" or \"switch_profile\"",
    )
    .await;
}
