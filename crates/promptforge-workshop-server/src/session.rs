//! The `/ws` WebSocket endpoint: one persistent socket carrying all
//! downstream JSON - browser chat over bidirectional text frames, relayed
//! through the gateway's streaming chat completion, plus unsolicited status
//! updates from the observer and model catalog pushes.
//!
//! A client upgrades `GET /ws` once and sends chat requests as text frames:
//! `{"type":"chat","id":N,"model":"...","messages":[...]}`. Each chat frame
//! runs one streaming gateway completion; the session answers with
//! `{"type":"delta","content":"...","id":N}` frames as content arrives,
//! `{"type":"reasoning","content":"...","id":N}` frames as the model's
//! reasoning side channel streams (the UI renders these as the Thinking
//! block), a terminal `{"type":"done","id":N}` when the stream completes, or
//! `{"type":"error","message":"...","id":N}` on any failure - transport,
//! mid-stream, or a gateway that declines the stream with a non-success
//! status. A frame that is not a well-formed chat request is answered
//! with an `error` frame and the session continues. A chat received while
//! the heartbeat knows the gateway is down is answered immediately with a
//! "Gateway unreachable" error frame - no upstream attempt, no tape event.
//!
//! Chats multiplex: the `id` is echoed verbatim on every frame of that
//! chat's reply, and distinct ids stream concurrently - frames of one
//! chat stay in stream order, different chats interleave freely on the
//! socket, one delta per frame. Frames without an `id` cannot be demuxed,
//! so untagged chats stay singular: at most one untagged chat streams at
//! a time, and a second is refused with an `error` frame naming the rule,
//! as is a chat reusing a live id. A `{"type":"cancel","id":N}` frame
//! tears down that one chat - dropping its gateway work, the pending
//! open or the payload stream, cancels the upstream completion and its
//! tape guard records the abandonment - while every other chat streams
//! on; a cancel naming no live chat is ignored with a debug log, because
//! a cancel racing its own `done` is normal. The session imposes no
//! concurrency cap of its own: the gateway's per-dominion queue is the
//! limiter, and per-delta scheduling keeps the socket fair. Waiting in
//! that queue is therefore an expected state, so a chat joins the
//! in-flight map the moment its request is posted and awaits the
//! gateway's answer as a non-blocking `Opening` entry of the same merged
//! poll: a queue parked at capacity never stalls the socket, and the
//! deltas of live chats, the bus pushes, and the cancel that would free
//! the slot all keep flowing.
//!
//! Beside chats, the socket carries the Model-menu events.
//! `{"type":"select_model","model":"..."}` selects the chat model: the
//! menu validates the id against the retained catalog and publishes the
//! fresh workbench snapshot, handled inline because a map lookup and a
//! broadcast send cost microseconds between two deltas; an unknown
//! model is refused with an `error` frame. `{"type":"switch_profile",
//! "name":"..."}` starts a gateway profile switch: `begin_switch`
//! publishes the pending snapshot (`switching` set, `chat_ready` false)
//! before the frame handler returns, and the switch itself runs on its
//! own task - it consumes the gateway's stage stream into determinate
//! status-bar progress, refetches the profile state and model catalog,
//! and settles the menu. A second switch while one runs is refused with
//! an `error` frame. Both events echo an `id` on their refusals when
//! the frame carried one, exactly as a chat's is.
//!
//! One task owns the socket: a single `select!` loop reads inbound frames
//! and writes every outbound frame itself - no outbox channel, no writer
//! task. The in-flight chats' gateway payloads arrive as one merged
//! branch of the same loop, polled round-robin so a hot stream cannot
//! monopolize the socket, and status updates from [`crate::status`],
//! catalog pushes from [`crate::catalog`], and workbench snapshots from
//! [`crate::menu`] keep flowing between deltas while chats stream. On
//! connect the session first sends the retained status, catalog, and
//! workbench snapshots, honoring the delivery contract's resend promise
//! (see [`crate::protocol`]) - the UI boots from this socket alone, with
//! zero HTTP state fetches; after that the buses forward as they
//! publish, and a session too slow to drain them skips ahead to the
//! newest snapshot rather than slowing the producers.
//!
//! Exactly one tape event is written per chat frame, after that chat's
//! stream settles and before its terminal frame is sent, so a client
//! holding `done` or `error` can trust the tape to hold the exchange. A
//! client that disconnects mid-stream drops every in-flight chat's guard,
//! and each tapes its own `client disconnected` note beside its partial
//! content. While the session lives, the idle status push fires when the
//! last in-flight chat settles, not after each one; on a disconnect each
//! abandoned chat's guard pushes idle after its own tape write - a
//! repeat of an idempotent Ready snapshot, accepted so the drop guards
//! stay independent of each other.

mod gateway_chat;
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
use crate::protocol::{ChatRequest, ErrorFrame, parse_chat_request};

use self::gateway_chat::{ChatKey, Chats, advance_chat, begin_chat, cancel_chat, next_event};
use self::log::SessionLog;
use self::menu::{select_model, start_switch};

/// Chat session ids for log correlation, handed out in connection order.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// Upgrades a `GET /ws` request to a WebSocket chat session. A foreign
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

/// Runs one chat session until the socket closes or fails: a single
/// `select!` loop owning the socket for both reading and writing.
async fn run_session(mut socket: WebSocket, state: AppState) {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    tracing::info!(session, "chat session opened");
    let _closed = SessionLog { session };
    let push = state.push();

    // Subscribe before snapshotting, so an update emitted between the two
    // arrives at least once; the possible duplicate is harmless because
    // status and catalog frames are complete snapshots.
    let mut status_rx = state.status().subscribe();
    let mut catalog_rx = state.catalog().subscribe();
    let mut menu_rx = state.menu().subscribe();
    // The delivery contract resends the current status, catalog, and
    // workbench snapshots on reconnect; the buses retain the newest copy
    // for exactly this send, so the UI boots with zero HTTP state fetches.
    if let Some(update) = state.status().latest()
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

    let mut chats = Chats::new();
    // The buses close only when the server state tears down; a closed bus
    // disables its branch rather than spinning the loop on `Closed`.
    let mut status_open = true;
    let mut catalog_open = true;
    let mut menu_open = true;

    loop {
        tokio::select! {
            // Biased, with the merged payload branch last: an in-flight
            // chat's payload stream against a local gateway is ready on
            // every poll, and an unbiased select could starve everything
            // behind it. Draining the buses first bounds their staleness
            // at one frame; reading inbound next keeps the socket read at
            // all times, so a cancel lands while streams run hot. Neither
            // can starve the payloads in turn, because bus producers push
            // and the client sends at human pace.
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
            // Inbound is read unconditionally: chats multiplex, so a later
            // frame never waits for an earlier chat's stream; a client
            // that vanishes mid-stream surfaces as a failed send on the
            // durable branch below.
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    handle_frame(&state, session, &text, &mut chats, &mut socket).await;
                }
                // Binary frames carry no chat meaning; pings and pongs are
                // answered by axum itself.
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_))) => {}
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    tracing::warn!(session, %error, "chat session socket failed");
                    break;
                }
            },
            // The durable path. Chat reply frames are direct per-request
            // replies: the gateway streams feeding them are owned by this
            // loop and fan out to nobody else, so sending them from this
            // branch preserves per-chat stream order and delivers exactly -
            // the contract's direct-reply case, which needs no Notify or
            // cursor because no shared transcript state exists to index.
            // Chats still waiting for gateway admission resolve here too,
            // as non-blocking entries of the same merged poll.
            (index, event) = next_event(&mut chats) => {
                if !advance_chat(&mut chats, index, event, &mut socket, &push, state.backoff())
                    .await
                {
                    break;
                }
            }
        }
    }
}

/// Handles one inbound text frame: a well-formed `chat` frame posts a
/// streamed completion and joins the in-flight map in `Opening` state
/// (the refusals that need no gateway round-trip - untagged collision,
/// duplicate id, malformed request, gateway known down - are answered
/// here, immediately), a `cancel` frame tears down the chat it names,
/// `select_model` and `switch_profile` drive the Model menu, and
/// anything else is answered with an `error` frame. Nothing here awaits
/// the gateway: the open resolves in the session loop's merged branch,
/// so a request parked in the gateway's admission queue never blocks
/// this socket.
async fn handle_frame(
    state: &AppState,
    session: u64,
    text: &str,
    chats: &mut Chats,
    socket: &mut WebSocket,
) {
    let frame: serde_json::Value = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(error) => {
            send_error(socket, None, format!("invalid JSON frame: {error}")).await;
            return;
        }
    };
    // The request id, echoed on every frame of this chat's reply so one
    // persistent socket can multiplex requests. Absent and null both mean
    // untagged.
    let id = frame.get("id").cloned().filter(|id| !id.is_null());
    let kind = frame.get("type").and_then(serde_json::Value::as_str);
    if kind == Some("cancel") {
        cancel_chat(session, id.as_ref(), chats, &state.push()).await;
        return;
    }
    if kind == Some("select_model") {
        select_model(state, id.as_ref(), &frame, socket).await;
        return;
    }
    if kind == Some("switch_profile") {
        start_switch(state, id.as_ref(), &frame, socket).await;
        return;
    }
    if kind != Some("chat") {
        send_error(
            socket,
            id.as_ref(),
            "unknown frame type; expected \"chat\", \"cancel\", \"select_model\", \
             or \"switch_profile\"",
        )
        .await;
        return;
    }
    let key = ChatKey::from_id(id.as_ref());
    if chats.is_live(&key) {
        send_error(socket, id.as_ref(), key.refusal()).await;
        return;
    }
    let request: ChatRequest = match parse_chat_request(frame.clone()) {
        Ok(request) => request,
        Err(error) => {
            send_error(
                socket,
                id.as_ref(),
                format!("invalid chat request: {error}"),
            )
            .await;
            return;
        }
    };
    // A gateway the heartbeat knows is down is not attempted: the chat
    // fails fast with a user-visible error instead of a transport error,
    // and nothing is taped because no exchange happened.
    if !state.health().is_reachable() {
        send_error(socket, id.as_ref(), "Gateway unreachable").await;
        return;
    }
    chats.insert(key, begin_chat(state, request, frame, id));
}

/// Sends one JSON text frame; a false return means the client is gone.
async fn send_frame<F: serde::Serialize>(socket: &mut WebSocket, frame: &F) -> bool {
    // Serializing the protocol frames cannot fail: strings, integers, and
    // JSON values only. A frame that somehow cannot serialize is skipped,
    // which is not a gone client.
    let Ok(text) = serde_json::to_string(frame) else {
        return true;
    };
    socket.send(Message::Text(text.into())).await.is_ok()
}

/// Sends one `error` frame carrying `message`, tagged with the request's
/// `id` when there is one, ignoring a dead client.
async fn send_error(
    socket: &mut WebSocket,
    id: Option<&serde_json::Value>,
    message: impl Into<String>,
) {
    let _ = send_frame(socket, &ErrorFrame::new(message.into(), id)).await;
}
