//! The `/ws` WebSocket endpoint: one persistent socket carrying the
//! workshop's downstream JSON - unsolicited status updates from the
//! observer, model catalog pushes, and workbench snapshots - plus the
//! inbound Model-menu events.
//!
//! A client upgrades `GET /ws` once. The socket carries the Model-menu
//! events inbound. `{"type":"select_model","model":"..."}` selects the
//! chat model: the menu validates the id against the retained catalog and
//! publishes the fresh workbench snapshot, handled inline because a map
//! lookup and a broadcast send cost microseconds; an unknown model is
//! refused with an `error` frame. `{"type":"switch_profile","name":"..."}`
//! starts a gateway profile switch: `begin_switch` publishes the pending
//! snapshot (`switching` set, `chat_ready` false) before the frame
//! handler returns, and the switch itself runs on its own task - it
//! consumes the gateway's stage stream into determinate status-bar
//! progress, refetches the profile state and model catalog, and settles
//! the menu. A second switch while one runs is refused with an `error`
//! frame. Both events echo an `id` on their refusals when the frame
//! carried one. A frame that is not a well-formed menu event is answered
//! with an `error` frame and the session continues. Chat itself lives on
//! the `/agents/ws` socket ([`crate::session_agents`]); this endpoint
//! carries no chat frames.
//!
//! One task owns the socket: a single `select!` loop reads inbound frames
//! and writes every outbound frame itself - no outbox channel, no writer
//! task. Status updates from [`crate::status`], catalog pushes from
//! [`crate::catalog`], and workbench snapshots from [`crate::menu`] flow
//! as they publish. On connect the session first sends the retained
//! status, catalog, and workbench snapshots, honoring the delivery
//! contract's resend promise (see [`crate::protocol`]) - the UI boots
//! from this socket alone, with zero HTTP state fetches; after that the
//! buses forward as they publish, and a session too slow to drain them
//! skips ahead to the newest snapshot rather than slowing the producers.

mod log;
mod menu;

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::cross_site;
use crate::error::AppError;
use crate::protocol::ErrorFrame;

use self::log::SessionLog;
use self::menu::{select_model, start_switch};

/// Session ids for log correlation, handed out in connection order.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// Upgrades a `GET /ws` request to a WebSocket session. A foreign
/// `Origin` is refused with 403: WS upgrades bypass Sec-Fetch in older
/// browsers, so the loopback allowlist in [`crate::cross_site`] guards the
/// upgrade itself.
pub(crate) async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !cross_site::origin_allowed(&headers) {
        return AppError::CrossSite.into_response();
    }
    ws.on_upgrade(move |socket| run_session(socket, state))
}

/// Runs one session until the socket closes or fails: a single `select!`
/// loop owning the socket for both reading and writing.
async fn run_session(mut socket: WebSocket, state: AppState) {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    tracing::info!(session, "workshop session opened");
    let _closed = SessionLog { session };

    // Subscribe before snapshotting, so an update emitted between the two
    // arrives at least once; the possible duplicate is harmless because
    // status and catalog frames are complete snapshots.
    let mut status_rx = state.status().subscribe();
    let mut catalog_rx = state.catalog().subscribe();
    let mut menu_rx = state.menu().subscribe();
    // The delivery contract resends the current status, catalog, and
    // workbench snapshots on reconnect; the buses retain the newest copy
    // for exactly this send, so the UI boots with zero HTTP state fetches.
    // The status line is the one exception: a retained heartbeat transition
    // ("Connected to gateway") describes a past moment, so the join line is
    // recomputed from the current probe instead of replayed stale.
    if let Some(update) = crate::heartbeat::join_status(state.status().latest(), state.health())
        && !send_frame(&mut socket, &update.frame()).await
    {
        return;
    }
    if let Some(catalog) = state.catalog().latest()
        && !send_frame(&mut socket, &catalog.frame()).await
    {
        return;
    }
    if let Some(snapshot) = state.menu().latest()
        && !send_frame(&mut socket, &snapshot.frame()).await
    {
        return;
    }

    // The buses close only when the server state tears down; a closed bus
    // disables its branch rather than spinning the loop on `Closed`.
    let mut status_open = true;
    let mut catalog_open = true;
    let mut menu_open = true;

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
            received = status_rx.recv(), if status_open => match received {
                Ok(update) => {
                    if !send_frame(&mut socket, &update.frame()).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(session, skipped, "status receiver lagged; skipped updates");
                }
                Err(broadcast::error::RecvError::Closed) => status_open = false,
            },
            received = catalog_rx.recv(), if catalog_open => match received {
                Ok(catalog) => {
                    if !send_frame(&mut socket, &catalog.frame()).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(session, skipped, "catalog receiver lagged; skipped pushes");
                }
                Err(broadcast::error::RecvError::Closed) => catalog_open = false,
            },
            received = menu_rx.recv(), if menu_open => match received {
                Ok(snapshot) => {
                    if !send_frame(&mut socket, &snapshot.frame()).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::debug!(session, skipped, "menu receiver lagged; skipped snapshots");
                }
                Err(broadcast::error::RecvError::Closed) => menu_open = false,
            },
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    handle_frame(&state, &text, &mut socket).await;
                }
                // Binary frames carry no meaning here; pings and pongs are
                // answered by axum itself.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_))) => {}
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    tracing::warn!(session, %error, "workshop session socket failed");
                    break;
                }
            },
        }
    }
}

/// Handles one inbound text frame: `select_model` and `switch_profile`
/// drive the Model menu, and anything else is answered with an `error`
/// frame. Refusals echo the frame's `id` when it carried one.
async fn handle_frame(state: &AppState, text: &str, socket: &mut WebSocket) {
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

/// Sends one JSON text frame; a false return means the client is gone.
/// Shared with the agent-session socket, its second production consumer.
pub(crate) async fn send_frame<F: serde::Serialize>(socket: &mut WebSocket, frame: &F) -> bool {
    // Serializing the protocol frames cannot fail: strings, integers, and
    // JSON values only. A frame that somehow cannot serialize is skipped,
    // which is not a gone client.
    let Ok(text) = serde_json::to_string(frame) else {
        return true;
    };
    socket.send(Message::Text(text.into())).await.is_ok()
}

/// Sends one `error` frame carrying `message`, tagged with the request's
/// `id` when there is one, ignoring a dead client. Shared with the
/// agent-session socket, its second production consumer.
pub(crate) async fn send_error(
    socket: &mut WebSocket,
    id: Option<&serde_json::Value>,
    message: impl Into<String>,
) {
    let _ = send_frame(socket, &ErrorFrame::new(message.into(), id)).await;
}
