//! The `/agents/ws` WebSocket endpoint: one socket serving one agent
//! session at a time.
//!
//! On connect the server pushes the discovered agent list. The client
//! then sends `{"type":"launch","agent":"..."}` to start a session or
//! `{"type":"attach","session":"..."}` to reattach to a running one -
//! sessions outlive sockets, so a reconnect replays the persisted event
//! log from index zero and re-announces every unresolved input wait.
//! While attached, the loop streams four families: durable
//! `agent_event` frames drained from the session's event log by a
//! per-client cursor (the log's broadcast is only the wakeup, so a
//! lagged receiver loses nothing), ephemeral `agent_delta` frames from
//! the session's delta channel (drops repair via the superseding event),
//! the durable `input_required` / `input_cancelled` wait frames, and
//! ephemeral `error` frames reporting a failed model round the program
//! survived or a run that ended in error.
//! `{"type":"input_response",...}` answers a wait and dispatches the
//! turn (the Thinking status push); `{"type":"cancel"}` fires the
//! session's turn-cancel - a stop reason, never an error, so nothing is
//! answered and the frames that follow are the relaunch's own.
//!
//! One task owns the socket: a single `select!` loop reads and writes
//! the same handle, per the crate's socket rule; the session registry
//! behind it is [`super`]'s documented carve-out.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use promptforge_core_support::events::{EventLog as _, RuntimeEvent};
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::cross_site;
use crate::error::AppError;
use crate::input::{WaitError, deliver_input_response};
use crate::protocol::{
    Activity, AgentDeltaFrame, AgentEventFrame, AgentSessionFrame, AgentsFrame, ErrorFrame,
    InputFrame, InputResponse,
};
use crate::session::{send_error, send_frame};

use super::{AgentDelta, AgentSession, reply_stamp};

/// The agent-session socket route.
pub(crate) fn routes(state: AppState) -> Router {
    Router::new()
        .route("/agents/ws", get(upgrade))
        .with_state(state)
}

/// Upgrades a `GET /agents/ws` request to an agent-session socket. A
/// foreign `Origin` is refused with 403, exactly as the chat socket's
/// upgrade is.
async fn upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !cross_site::origin_allowed(&headers) {
        return AppError::CrossSite.into_response();
    }
    ws.on_upgrade(move |socket| run_socket(socket, state))
}

/// The attachment state of one socket: the session it serves and the
/// per-client cursors deriving durable-frame indices and reply stamps.
struct Attached {
    /// The session this socket serves.
    session: Arc<AgentSession>,
    /// The next event-log index to send; everything below it has been
    /// framed to this client already.
    cursor: u64,
    /// Settled model rounds seen at the cursor - the socket-side half of
    /// the reply-stamp rule ([`reply_stamp`]).
    rounds_seen: u64,
}

/// Receives from an optional subscription, pending forever when absent,
/// so a `select!` branch for a detached channel simply never fires.
async fn recv_or_pending<T: Clone>(
    receiver: &mut Option<broadcast::Receiver<T>>,
) -> Result<T, broadcast::error::RecvError> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// Runs one agent-session socket until it closes or fails.
async fn run_socket(mut socket: WebSocket, state: AppState) {
    // The list is discovered per connect: the frame is a complete
    // snapshot, so a directory edited between connects is picked up by
    // the next window with no push machinery.
    if !send_frame(&mut socket, &AgentsFrame::new(state.agents().discover())).await {
        return;
    }
    let mut attached: Option<Attached> = None;
    // The subscriptions ride beside the attachment (not inside it) so the
    // select! arms below can borrow them while the inbound arm borrows
    // `attached`; attach() and the arms keep them all in step.
    let mut events_rx: Option<broadcast::Receiver<RuntimeEvent>> = None;
    let mut deltas_rx: Option<broadcast::Receiver<AgentDelta>> = None;
    let mut input_rx: Option<broadcast::Receiver<InputFrame>> = None;
    let mut errors_rx: Option<broadcast::Receiver<String>> = None;

    loop {
        tokio::select! {
            // Biased, in this order: error reports first - one-off and
            // causally ahead of the wait that follows a failed round, so
            // the error frame precedes the re-ask on the wire; the wait
            // frames next, tiny and rare, so they never grow stale;
            // inbound next keeps the socket read at all times, so a
            // cancel lands while a stream runs hot; deltas before the
            // event drain, so when a whole round sits queued the chunks
            // flush before the durable event that supersedes them; the
            // event drain last loses nothing, because the cursor
            // delivers everything past it whenever it runs.
            biased;
            // Session errors: ephemeral - a lagged receiver misses only
            // what the durable transcript shows as a turn with no reply.
            received = recv_or_pending(&mut errors_rx) => {
                match received {
                    Ok(message) => {
                        if !send_frame(&mut socket, &ErrorFrame::new(message, None)).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "agent error receiver lagged; reports dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => errors_rx = None,
                }
            }
            // Durable wait frames: the registry retains unresolved waits,
            // so a lagged receiver repairs by re-announcing them.
            received = recv_or_pending(&mut input_rx) => {
                match received {
                    Ok(frame) => {
                        if !send_frame(&mut socket, &frame).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(attached) = attached.as_ref()
                            && !resend_unresolved(attached, &mut socket).await
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => input_rx = None,
                }
            }
            inbound = socket.recv() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    let outcome = handle_frame(
                        &state,
                        &text,
                        &mut attached,
                        (&mut events_rx, &mut deltas_rx, &mut input_rx, &mut errors_rx),
                        &mut socket,
                    )
                    .await;
                    if !outcome {
                        break;
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_))) => {}
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    tracing::warn!(%error, "agent session socket failed");
                    break;
                }
            },
            // Ephemeral deltas: a lagged client skips chunks and the
            // superseding durable event repairs the transcript.
            received = recv_or_pending(&mut deltas_rx) => {
                match received {
                    Ok(delta) => {
                        let frame =
                            AgentDeltaFrame::new(delta.channel, delta.content, delta.reply);
                        if !send_frame(&mut socket, &frame).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "agent delta receiver lagged; chunks dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => deltas_rx = None,
                }
            }
            // Durable events: the broadcast is only the wakeup - the
            // frames are drained from the log by cursor, so a lagged or
            // even closed receiver never loses an entry.
            received = recv_or_pending(&mut events_rx) => {
                match received {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(attached) = attached.as_mut()
                            && !drain_events(attached, &mut socket).await
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => events_rx = None,
                }
            }
        }
    }
    // The socket detaches; the session lives on. Reconnecting replays the
    // log and re-announces unresolved waits.
}

/// The four channel subscriptions an attachment holds, passed as one
/// bundle so [`handle_frame`] can replace them atomically on attach.
type Subscriptions<'a> = (
    &'a mut Option<broadcast::Receiver<RuntimeEvent>>,
    &'a mut Option<broadcast::Receiver<AgentDelta>>,
    &'a mut Option<broadcast::Receiver<InputFrame>>,
    &'a mut Option<broadcast::Receiver<String>>,
);

/// Handles one inbound text frame. A `false` return means the client is
/// gone and the socket loop should end.
async fn handle_frame(
    state: &AppState,
    text: &str,
    attached: &mut Option<Attached>,
    subscriptions: Subscriptions<'_>,
    socket: &mut WebSocket,
) -> bool {
    let frame: serde_json::Value = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(error) => {
            send_error(socket, None, format!("invalid JSON frame: {error}")).await;
            return true;
        }
    };
    match frame.get("type").and_then(serde_json::Value::as_str) {
        Some(kind @ ("launch" | "attach")) => {
            handle_open(state, kind, &frame, attached, subscriptions, socket).await
        }
        Some("input_response") => {
            let Some(attached) = attached.as_ref() else {
                send_error(socket, None, "input_response before a session is attached").await;
                return true;
            };
            let response: InputResponse = match serde_json::from_value(frame.clone()) {
                Ok(response) => response,
                Err(error) => {
                    send_error(socket, None, format!("invalid input_response: {error}")).await;
                    return true;
                }
            };
            let session = &attached.session;
            match deliver_input_response(
                session.log.as_ref(),
                &session.waits,
                &session.id,
                &session.agent,
                response,
            ) {
                // The wait completed: the turn is dispatched.
                Ok(()) => state.push().push_status_update(
                    "Running agent turn",
                    format!("agent `{}` is thinking", session.agent),
                    Activity::Thinking,
                ),
                // A response racing a turn-cancel is normal: the text is
                // recorded as history (the relaunched agent rebuilds from
                // events), and the dead wait already announced its
                // `input_cancelled`.
                Err(WaitError::UnknownToken) => {
                    tracing::debug!(
                        session = %session.id,
                        "input_response for a dead wait; text recorded, wait gone"
                    );
                }
            }
            true
        }
        Some("cancel") => {
            if let Some(attached) = attached.as_ref() {
                // Cancellation is a stop reason: no reply frame of any
                // kind. Pending waits announce their own deaths and the
                // relaunched run re-asks.
                attached.session.cancel_turn();
            } else {
                send_error(socket, None, "cancel before a session is attached").await;
            }
            true
        }
        _ => {
            send_error(
                socket,
                None,
                "unknown frame type; expected \"launch\", \"attach\", \"input_response\", \
                 or \"cancel\"",
            )
            .await;
            true
        }
    }
}

/// Handles a `launch` or `attach` frame: resolves the session it names
/// and attaches the socket to it. One socket serves one session - agent
/// windows are modal - so a second open on an attached socket is
/// refused. A `false` return means the client is gone.
async fn handle_open(
    state: &AppState,
    kind: &str,
    frame: &serde_json::Value,
    attached: &mut Option<Attached>,
    subscriptions: Subscriptions<'_>,
    socket: &mut WebSocket,
) -> bool {
    if attached.is_some() {
        send_error(
            socket,
            None,
            "this socket already serves a session; agent windows are modal",
        )
        .await;
        return true;
    }
    let session = if kind == "launch" {
        let Some(agent) = frame.get("agent").and_then(serde_json::Value::as_str) else {
            send_error(socket, None, "launch frame without an agent name").await;
            return true;
        };
        match state.agents().launch(agent) {
            Ok(session) => session,
            Err(refusal) => {
                send_error(socket, None, refusal.to_string()).await;
                return true;
            }
        }
    } else {
        let Some(id) = frame.get("session").and_then(serde_json::Value::as_str) else {
            send_error(socket, None, "attach frame without a session id").await;
            return true;
        };
        let Some(session) = state.agents().get(id) else {
            send_error(socket, None, "unknown agent session").await;
            return true;
        };
        session
    };
    attach(session, attached, subscriptions, socket).await
}

/// Attaches the socket to `session`: subscribes the three channels
/// (before the replay, so nothing lands between them unseen),
/// acknowledges with the session frame, replays the persisted log from
/// index zero, and re-announces unresolved waits. A `false` return means
/// the client is gone.
async fn attach(
    session: Arc<AgentSession>,
    attached: &mut Option<Attached>,
    (events_rx, deltas_rx, input_rx, errors_rx): Subscriptions<'_>,
    socket: &mut WebSocket,
) -> bool {
    *events_rx = Some(session.log.subscribe());
    *deltas_rx = Some(session.subscribe_deltas());
    *input_rx = Some(session.input_frames.subscribe());
    *errors_rx = Some(session.subscribe_errors());
    let acknowledgment = AgentSessionFrame::new(session.id.clone(), session.agent.clone());
    let mut state = Attached {
        session,
        cursor: 0,
        rounds_seen: 0,
    };
    if !send_frame(socket, &acknowledgment).await
        || !drain_events(&mut state, socket).await
        || !resend_unresolved(&state, socket).await
    {
        return false;
    }
    *attached = Some(state);
    true
}

/// Sends every log entry past the client's cursor as a durable
/// `agent_event` frame carrying its log index and, on the model-round
/// content kinds, the reply stamp its deltas carried. A `false` return
/// means the client is gone.
async fn drain_events(attached: &mut Attached, socket: &mut WebSocket) -> bool {
    let len = attached.session.log.len();
    while attached.cursor < len {
        let Some(event) = attached.session.log.get(attached.cursor) else {
            // Unreachable: the log is append-only, so every index below
            // a witnessed len() reads. Stop cleanly rather than spin.
            return true;
        };
        let stamp = reply_stamp(event.kind, &mut attached.rounds_seen);
        let frame = AgentEventFrame::new(attached.cursor, stamp, event);
        if !send_frame(socket, &frame).await {
            return false;
        }
        attached.cursor += 1;
    }
    true
}

/// Re-announces every unresolved wait to this socket in creation order -
/// the attach-time (and lag-repair) half of the durable input-frame
/// promise. A `false` return means the client is gone.
async fn resend_unresolved(attached: &Attached, socket: &mut WebSocket) -> bool {
    for token in attached.session.waits.unresolved() {
        if !send_frame(socket, &InputFrame::Required { token }).await {
            return false;
        }
    }
    true
}
