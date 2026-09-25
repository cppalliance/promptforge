//! The `/agents/ws` WebSocket endpoint: one socket serving one agent
//! session at a time.
//!
//! On connect the server pushes the discovered agent list. The client
//! then sends `{"type":"launch","agent":"..."}` to start a session or
//! `{"type":"attach","session":"..."}` to reattach to a running one -
//! sessions outlive sockets, so a reconnect replays the session's
//! transcript from index zero and re-announces every unresolved input
//! wait. While attached, the loop streams four families: durable
//! `agent_event` frames drained from the session's transcript by a
//! per-client cursor (the harness's event broadcast is only the wakeup,
//! so a lagged receiver loses nothing), ephemeral `agent_delta` frames
//! from the session's delta channel (drops repair via the superseding
//! event), the durable `input_required` / `input_cancelled` wait frames,
//! and ephemeral `error` frames reporting a failed model round the
//! program survived or a run that ended in error.
//! `{"type":"input_response",...}` answers a wait and dispatches the
//! turn (the Thinking status push); `{"type":"cancel"}` fires the
//! session's turn-cancel - a stop reason, never an error, so nothing is
//! answered and the frames that follow are the relaunch's own.
//!
//! One task owns the socket: a single `select!` loop reads and writes
//! the same handle, per the server's socket rule; the session table
//! behind it is the harness's, [`super`]'s documented carve-out.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use harness_api::{
    Delta, Session, SessionEvent, SessionFailure, WaitError, WaitFrame, display_chain,
};
use tokio::sync::broadcast;

use workshop_protocol::{
    Activity, AgentSessionFrame, AgentsFrame, ErrorFrame, InputFrame, InputResponse,
};

use super::LaunchRefusal;
use super::session::{cross_site_refusal, send_error, send_frame};
use super::socket_frames::{delta_frame, drain_events, frame_entry, input_frame};
use super::state::SessionsState;

/// Upgrades a `GET /agents/ws` request to an agent-session socket. A
/// foreign `Origin` is refused with 403, as the workbench socket's
/// upgrade is.
pub(crate) async fn upgrade(
    State(state): State<SessionsState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.origin_allowed(&headers) {
        return cross_site_refusal();
    }
    ws.on_upgrade(move |socket| run_socket(socket, state))
}

/// The attachment state of one socket: the session it serves and the
/// per-client cursors deriving durable-frame indices.
pub(crate) struct Attached {
    /// The session this socket serves.
    pub(crate) session: Session,
    /// The next transcript index to consider; everything below it has
    /// been read (framed or skipped) for this client already.
    pub(crate) cursor: u64,
    /// The wire index the next framed entry takes: the count of
    /// transcript entries with a wire shape sent so far, so the durable
    /// frames number the transcript the client renders, gap-free.
    pub(crate) framed: u64,
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
async fn run_socket(mut socket: WebSocket, state: SessionsState) {
    // The list is discovered per connect: the frame is a complete
    // snapshot, so a directory edited between connects is picked up by
    // the next window with no push machinery. An unregistered sessions
    // handle degrades the discovery to the empty list.
    let discovered = state
        .agents()
        .map_or_else(Vec::new, |agents| agents.discover());
    if !send_frame(&mut socket, &AgentsFrame::new(discovered)).await {
        return;
    }
    let mut attached: Option<Attached> = None;
    // The subscriptions are stored beside the attachment (not inside it)
    // so the select! arms below can borrow them while the inbound arm
    // borrows `attached`; attach() and the arms keep them all in step.
    let mut events_rx: Option<broadcast::Receiver<SessionEvent>> = None;
    let mut deltas_rx: Option<broadcast::Receiver<Delta>> = None;
    let mut input_rx: Option<broadcast::Receiver<WaitFrame>> = None;
    let mut errors_rx: Option<broadcast::Receiver<SessionFailure>> = None;

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
                    Ok(failure) => {
                        if !send_frame(&mut socket, &ErrorFrame::new(failure.message, None)).await {
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
                        if !send_frame(&mut socket, &input_frame(frame)).await {
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
                        if let Some(frame) = delta_frame(delta)
                            && !send_frame(&mut socket, &frame).await
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "agent delta receiver lagged; chunks dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => deltas_rx = None,
                }
            }
            // Durable events: the broadcast is only the wakeup - a live
            // entry at the cursor frames directly, and anything else
            // (a gap, a lag, even a closed receiver) drains the transcript
            // from the cursor, so no entry is ever lost.
            received = recv_or_pending(&mut events_rx) => match received {
                Err(broadcast::error::RecvError::Closed) => events_rx = None,
                received => {
                    if let Some(attached) = attached.as_mut()
                        && !on_event_wake(attached, received.ok(), &mut socket).await
                    {
                        break;
                    }
                }
            },
        }
    }
    // The socket detaches; the session lives on. Reconnecting replays the
    // transcript and re-announces unresolved waits.
}

/// The four channel subscriptions an attachment holds, passed as one
/// bundle so [`handle_frame`] can replace them atomically on attach.
type Subscriptions<'a> = (
    &'a mut Option<broadcast::Receiver<SessionEvent>>,
    &'a mut Option<broadcast::Receiver<Delta>>,
    &'a mut Option<broadcast::Receiver<WaitFrame>>,
    &'a mut Option<broadcast::Receiver<SessionFailure>>,
);

/// Handles one inbound text frame. A `false` return means the client is
/// gone and the socket loop should end.
async fn handle_frame(
    state: &SessionsState,
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
            match session.send_input(&response.token, response.text, || {}) {
                // The wait completed: the turn is dispatched.
                Ok(()) => state.push().push_status_update(
                    "Running agent turn",
                    format!("agent `{}` is thinking", session.agent()),
                    Activity::Thinking,
                ),
                // A response racing a turn-cancel is normal: the dead wait
                // already announced its `input_cancelled`, and the
                // relaunched agent re-asks.
                Err(WaitError::UnknownToken) => {
                    tracing::debug!(
                        session = %session.id(),
                        "input_response for a dead wait; wait gone"
                    );
                }
            }
            true
        }
        Some("cancel") => {
            if let Some(attached) = attached.as_ref() {
                // Cancellation is a stop reason: no reply frame of any
                // kind. Pending waits announce their own deaths and the
                // relaunched run re-asks. The relaunch reads the host
                // snapshot, so the server's current state is pushed first.
                if let Some(agents) = state.agents() {
                    agents.sync_bindings();
                }
                attached.session.cancel();
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
    state: &SessionsState,
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
    let Some(agents) = state.agents() else {
        send_error(socket, None, "agent sessions are unavailable").await;
        return true;
    };
    let session = if kind == "launch" {
        let Some(agent) = frame.get("agent").and_then(serde_json::Value::as_str) else {
            send_error(socket, None, "launch frame without an agent name").await;
            return true;
        };
        match agents.launch(agent).await {
            Ok(session) => session,
            Err(refusal) => {
                send_error(socket, None, refusal_text(&refusal)).await;
                return true;
            }
        }
    } else {
        let Some(id) = frame.get("session").and_then(serde_json::Value::as_str) else {
            send_error(socket, None, "attach frame without a session id").await;
            return true;
        };
        let Some(session) = agents.get(id) else {
            send_error(socket, None, "unknown agent session").await;
            return true;
        };
        session
    };
    attach(session, attached, subscriptions, socket).await
}

/// The text of the error frame reporting a refused launch: the refusal
/// and its cause chain. A refusal's `Display` is only its own
/// message, so a run log that cannot open would otherwise reach the
/// client as the bare "run log database" with the engine's diagnosis gone.
fn refusal_text(refusal: &LaunchRefusal) -> String {
    display_chain(refusal)
}

/// Attaches the socket to `session`: subscribes the four channels
/// (before the replay, so nothing lands between them unseen),
/// acknowledges with the session frame, replays the session's
/// transcript from index zero, and re-announces unresolved waits. A
/// `false` return means the client is gone.
async fn attach(
    session: Session,
    attached: &mut Option<Attached>,
    (events_rx, deltas_rx, input_rx, errors_rx): Subscriptions<'_>,
    socket: &mut WebSocket,
) -> bool {
    *events_rx = Some(session.subscribe_events());
    *deltas_rx = Some(session.subscribe_deltas());
    *input_rx = Some(session.subscribe_waits());
    *errors_rx = Some(session.subscribe_errors());
    let acknowledgment =
        AgentSessionFrame::new(session.id().to_string(), session.agent().to_owned());
    let mut state = Attached {
        session,
        cursor: 0,
        framed: 0,
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

/// Frames what an event wakeup delivered: the live entry itself when it
/// is the entry at the cursor (or one the replay already covered, which
/// frames nothing), else - a gap past the cursor, or a lag that delivered
/// no entry - the transcript from the cursor on. A `false` return means
/// the client is gone.
async fn on_event_wake(
    attached: &mut Attached,
    entry: Option<SessionEvent>,
    socket: &mut WebSocket,
) -> bool {
    match entry {
        Some(entry) if entry.index <= attached.cursor => {
            frame_entry(attached, &entry, socket).await
        }
        _ => drain_events(attached, socket).await,
    }
}

/// Re-announces every unresolved wait to this socket in creation order -
/// the attach-time (and lag-repair) half of the durable input-frame
/// promise. A `false` return means the client is gone.
async fn resend_unresolved(attached: &Attached, socket: &mut WebSocket) -> bool {
    for token in attached.session.unresolved_waits() {
        if !send_frame(socket, &InputFrame::Required { token }).await {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[path = "socket-tests.rs"]
mod tests;
