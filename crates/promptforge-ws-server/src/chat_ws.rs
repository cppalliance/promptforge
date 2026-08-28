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

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;
use std::time::Instant;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use futures_util::{FutureExt, StreamExt};
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::backoff::ReconnectBackoff;
use crate::cross_site;
use crate::error::AppError;
use crate::gateway::{
    ChatStream, GatewayClient, GatewayError, GatewayResponse, SsePayloadStream, SwitchEvent,
    SwitchResponse, switch_events,
};
use crate::heartbeat::{refresh_catalog, refresh_profiles};
use crate::menu::SwitchOutcome;
use crate::protocol::{Activity, ChatRequest, DeltaFrame, DoneFrame, ErrorFrame, ReasoningFrame};
use crate::push::Push;
use crate::relay::{tape_round_trip, value_from_bytes};
use crate::tape::Tape;

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

/// Logs the session close when the connection task ends, however it ends,
/// so the session loop's exit paths carry no cleanup calls.
struct SessionLog {
    session: u64,
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        tracing::info!(session = self.session, "chat session closed");
    }
}

/// One chat in flight - one entry of the session's chat map: the
/// gateway work in whichever phase it is in, beside the state the settle
/// paths need. Everything here releases on drop - dropping the work
/// cancels the upstream completion (a pending open aborts its HTTP
/// request, a stream closes its body), and the tape guard records the
/// abandoned exchange.
struct ActiveChat {
    work: ChatWork,
    /// The request's `id`, echoed on every frame of this chat's reply.
    id: Option<serde_json::Value>,
    tape: StreamTape,
}

/// The gateway side of one in-flight chat, in lifecycle order. A chat
/// spends its whole admission wait in `Opening` - the expected state
/// while the gateway's per-dominion queue is at capacity - and the
/// session loop keeps polling everything else meanwhile, because both
/// phases are driven by the same merged [`next_event`] branch.
enum ChatWork {
    /// The completion is posted but the gateway has not answered its
    /// response headers yet. Dropping the future abandons the request:
    /// reqwest cancels the in-flight HTTP exchange on drop, which frees
    /// the gateway's queue slot.
    Opening(BoxFuture<'static, Result<ChatStream, GatewayError>>),
    /// The gateway accepted the stream; SSE payloads arrive in order.
    Streaming(SsePayloadStream),
}

/// The demux key of one in-flight chat. Reply frames without an `id`
/// cannot be told apart on the wire, so all untagged chats share one
/// reserved slot and at most one streams at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ChatKey {
    /// The single chat allowed to stream without an `id`.
    Untagged,
    /// A chat named by its request `id`, keyed by the id's JSON text.
    Tagged(String),
}

impl ChatKey {
    /// Derives the key from a request's optional `id`.
    fn from_id(id: Option<&serde_json::Value>) -> Self {
        match id {
            Some(id) => Self::Tagged(id.to_string()),
            None => Self::Untagged,
        }
    }

    /// The refusal sent for a chat frame that collides with this live
    /// key, naming the rule it broke.
    fn refusal(&self) -> String {
        match self {
            Self::Untagged => "an untagged chat is already streaming; frames without an id \
                               cannot be demuxed, so at most one untagged chat streams at a time"
                .to_string(),
            Self::Tagged(id) => {
                format!(
                    "a chat with id {id} is already streaming; each concurrent chat needs its own id"
                )
            }
        }
    }
}

/// The session's in-flight chats: the demux map the loop selects over,
/// with a rotating poll cursor for per-delta fairness.
struct Chats {
    /// In-flight chats in arrival order. A `Vec` rather than a keyed map:
    /// the population is a handful of UI tabs, and round-robin polling
    /// wants stable positions more than fast lookups.
    entries: Vec<(ChatKey, ActiveChat)>,
    /// Where the next merged poll starts, advanced past each chat that
    /// yields, so an always-ready stream cannot starve its neighbors.
    cursor: usize,
}

impl Chats {
    /// An empty map: no chat in flight.
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
        }
    }

    /// Whether no chat is in flight.
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a chat with this key is streaming right now.
    fn is_live(&self, key: &ChatKey) -> bool {
        self.entries.iter().any(|(live, _)| live == key)
    }

    /// Adds a newly opened chat to the map.
    fn insert(&mut self, key: ChatKey, chat: ActiveChat) {
        self.entries.push((key, chat));
    }

    /// Removes and returns the chat with this key, if one is in flight.
    fn remove(&mut self, key: &ChatKey) -> Option<ActiveChat> {
        let index = self.entries.iter().position(|(live, _)| live == key)?;
        Some(self.entries.remove(index).1)
    }
}

/// One step of one in-flight chat, yielded by the merged poll.
enum ChatEvent {
    /// The chat's open resolved: the gateway answered the request's
    /// headers, declined it, or the request failed in transport.
    Opened(Result<ChatStream, GatewayError>),
    /// One item of the chat's payload stream; `None` is the terminal
    /// marker - the stream ended and the caller runs the settle path.
    Payload(Option<Result<String, GatewayError>>),
}

/// The next event across every in-flight chat, paired with the map index
/// of the chat that yielded it - the merged future the session loop
/// selects over. An `Opening` chat yields its resolved open; a
/// `Streaming` chat yields payloads in stream order. Distinct chats are
/// polled round-robin from a rotating cursor, one event per turn, so
/// neither a hot stream nor a parked admission can starve its neighbors.
/// Pending forever while no chat is in flight, so the select! branch
/// simply never fires.
async fn next_event(chats: &mut Chats) -> (usize, ChatEvent) {
    std::future::poll_fn(|context| {
        let count = chats.entries.len();
        for step in 0..count {
            let index = (chats.cursor + step) % count;
            let event = match &mut chats.entries[index].1.work {
                ChatWork::Opening(open) => match open.poll_unpin(context) {
                    Poll::Ready(outcome) => ChatEvent::Opened(outcome),
                    Poll::Pending => continue,
                },
                ChatWork::Streaming(payloads) => match payloads.poll_next_unpin(context) {
                    Poll::Ready(item) => ChatEvent::Payload(item),
                    Poll::Pending => continue,
                },
            };
            chats.cursor = (index + 1) % count;
            return Poll::Ready((index, event));
        }
        Poll::Pending
    })
    .await
}

/// Advances the chat at `index` by one event: transitions a resolved
/// open into its stream (or settles it on refusal or failure), forwards
/// deltas, settles the chat when its stream ends or fails. A `false`
/// return means the client is gone and the session loop should end;
/// every abandoned chat's guard then tapes the disconnect.
async fn advance_chat(
    chats: &mut Chats,
    index: usize,
    event: ChatEvent,
    socket: &mut WebSocket,
    push: &Push,
    backoff: &ReconnectBackoff,
) -> bool {
    let payload = match event {
        ChatEvent::Opened(Ok(ChatStream::Stream { payloads, .. })) => {
            push.push_status_update(
                "Streaming response...",
                "the gateway is streaming the reply",
                Activity::Thinking,
            );
            chats.entries[index].1.work = ChatWork::Streaming(payloads);
            return true;
        }
        ChatEvent::Opened(Ok(ChatStream::Relay(upstream))) => {
            let ActiveChat { id, tape, .. } = chats.entries.remove(index).1;
            declined_stream(tape, upstream, id.as_ref(), push, socket).await;
            return true;
        }
        ChatEvent::Opened(Err(error)) => {
            let ActiveChat { id, tape, .. } = chats.entries.remove(index).1;
            // No response ever arrived, so no exchange happened and
            // nothing is taped - the same no-tape rule as a chat the
            // heartbeat short-circuits.
            tape.discard();
            let message = error.to_string();
            push.push_failure("Connection lost", message.clone(), Activity::General);
            send_error(socket, id.as_ref(), message).await;
            return true;
        }
        ChatEvent::Payload(payload) => payload,
    };
    match payload {
        Some(Ok(payload)) => {
            // The terminal sentinel ends the wire stream but carries no
            // content; role-priming and usage events have none either.
            if payload == "[DONE]" {
                return true;
            }
            match forward_payload(&payload, &mut chats.entries[index].1, socket, push, backoff)
                .await
            {
                Forward::Sent => true,
                Forward::ClientGone => false,
            }
        }
        Some(Err(error)) => {
            let ActiveChat { id, tape, .. } = chats.entries.remove(index).1;
            let message = error.to_string();
            tape.record(Some(message.clone())).await;
            push.push_failure("Connection lost", message.clone(), Activity::General);
            send_error(socket, id.as_ref(), message).await;
            true
        }
        None => {
            let ActiveChat { id, tape, .. } = chats.entries.remove(index).1;
            tape.record(None).await;
            // The idle push waits for the last settle: while other chats
            // still stream, the bar keeps reporting their activity.
            if chats.is_empty() {
                push.push_idle();
            }
            let _ = send_frame(socket, &DoneFrame::new(id.as_ref())).await;
            true
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
    let request: ChatRequest = match serde_json::from_value(frame.clone()) {
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

/// Tears down the one chat a `cancel` frame names: dropping its gateway
/// work cancels the upstream completion - a streaming chat closes its
/// payload stream, a chat still waiting for admission aborts its queued
/// request - and its tape records the abandonment beside the partial
/// content (empty for a chat that never streamed) - the same teardown a
/// disconnect performs, scoped to one chat. A cancel for an unknown or
/// already-settled chat is ignored with a debug log, because a cancel
/// racing its own `done` is normal.
async fn cancel_chat(session: u64, id: Option<&serde_json::Value>, chats: &mut Chats, push: &Push) {
    let key = ChatKey::from_id(id);
    let Some(active) = chats.remove(&key) else {
        tracing::debug!(
            session,
            ?key,
            "cancel for an unknown or settled chat; ignored"
        );
        return;
    };
    let ActiveChat { work, tape, .. } = active;
    // The upstream completion dies before the tape write, exactly as it
    // does when a disconnect drops the whole map.
    drop(work);
    tape.record(Some("chat canceled by client".to_string()))
        .await;
    // A cancel that ends the last in-flight chat is the last settle.
    if chats.is_empty() {
        push.push_idle();
    }
}

/// Handles a `select_model` frame: the menu validates the id against
/// the retained catalog and publishes the fresh workbench snapshot,
/// which reaches this client through its own menu branch. Handled
/// inline in the session loop - a map lookup and a broadcast send cost
/// microseconds between two deltas. A refusal (an unknown model, a
/// missing field) is answered with an `error` frame and the session
/// continues (zone two).
async fn select_model(
    state: &AppState,
    id: Option<&serde_json::Value>,
    frame: &serde_json::Value,
    socket: &mut WebSocket,
) {
    let Some(model) = frame.get("model").and_then(serde_json::Value::as_str) else {
        send_error(socket, id, "select_model needs a \"model\" string").await;
        return;
    };
    if let Err(refusal) = state.menu().set_selected(model) {
        send_error(socket, id, refusal.to_string()).await;
    }
}

/// Handles a `switch_profile` frame: `begin_switch` publishes the
/// pending snapshot (`switching` set, `chat_ready` false) before this
/// returns, and the switch itself runs on its own task. A refusal (a
/// switch already in flight, a missing field) is answered with an
/// `error` frame and the session continues (zone two).
async fn start_switch(
    state: &AppState,
    id: Option<&serde_json::Value>,
    frame: &serde_json::Value,
    socket: &mut WebSocket,
) {
    let Some(name) = frame.get("name").and_then(serde_json::Value::as_str) else {
        send_error(socket, id, "switch_profile needs a \"name\" string").await;
        return;
    };
    if let Err(refusal) = state.menu().begin_switch(name) {
        send_error(socket, id, refusal.to_string()).await;
        return;
    }
    // Deliberately not client-scoped - a stated exception to the crate's
    // drop-guard cancellation rule: a profile switch is global server
    // state, not work held on behalf of one client, so it runs to
    // completion (and settles the menu) even if the clicking client
    // disconnects mid-switch.
    let client = state.gateway_client().clone();
    let push = state.push();
    let name = name.to_string();
    tokio::spawn(async move {
        run_switch(&client, &push, &name).await;
    });
}

/// How many stage markers the gateway's switch stream emits, in
/// execution order: `loading-profile`, `stopping-models`,
/// `starting-models`.
const SWITCH_STAGES: u64 = 3;

/// Runs one profile switch to its end: drives the gateway's stage
/// stream into determinate status-bar progress, refetches the profile
/// state and model catalog, and settles the menu - the remembered model
/// selected and `chat_ready` recomputed on success, the truthful
/// pre-switch state restored on failure - before pushing the idle or
/// failure status.
async fn run_switch(client: &GatewayClient, push: &Push, name: &str) {
    let outcome = drive_switch(client, push, name).await;
    // The gateway's serving state may have changed even on a failed
    // switch (its documented degraded state can lose local children),
    // so both paths refetch before the menu settles; the final
    // workbench snapshot then reads a fresh catalog and profile list.
    tokio::join!(
        refresh_profiles(client, push),
        refresh_catalog(client, push)
    );
    match outcome {
        Ok(()) => {
            push.menu().finish_switch(SwitchOutcome::Completed);
            push.push_idle();
        }
        Err(message) => {
            push.menu().finish_switch(SwitchOutcome::Failed);
            push.push_failure("Profile switch failed", message, Activity::General);
        }
    }
}

/// Posts the switch and consumes its stage stream, pushing each stage
/// marker as determinate progress, until the terminal event: `Ok` on
/// `ready`, the failure's description on everything else - a terminal
/// `error`, a buffered refusal, a transport failure, or a stream that
/// ends without a terminal event.
async fn drive_switch(client: &GatewayClient, push: &Push, name: &str) -> Result<(), String> {
    let payloads = match client.switch_profile(name).await {
        Ok(SwitchResponse::Switching { payloads, .. }) => payloads,
        Ok(SwitchResponse::Buffered(refusal)) => return Err(switch_refusal(&refusal)),
        Err(error) => return Err(error.to_string()),
    };
    let mut events = switch_events(payloads);
    while let Some(item) = events.next().await {
        match item {
            Ok(SwitchEvent::Stage { stage }) => push_stage(push, name, &stage),
            Ok(SwitchEvent::Ready { .. }) => return Ok(()),
            Ok(SwitchEvent::Error { message }) => return Err(message),
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("the switch stream ended without a terminal event".to_string())
}

/// Pushes one stage marker as determinate status-bar progress - stage
/// n of [`SWITCH_STAGES`] with a label naming the stage. A stage this
/// build does not know is logged and skipped: the gateway may grow
/// stages, and a lost progress update never degrades the switch
/// (zone two).
fn push_stage(push: &Push, name: &str, stage: &str) {
    let (label, current) = match stage {
        "loading-profile" => ("Loading profile...", 1),
        "stopping-models" => ("Stopping models...", 2),
        "starting-models" => ("Starting models...", 3),
        other => {
            tracing::debug!(stage = other, "unknown switch stage; no progress pushed");
            return;
        }
    };
    push.push_progress(
        label,
        format!("switching to profile {name}"),
        current,
        SWITCH_STAGES,
        Activity::General,
    );
}

/// The failure description of a buffered switch refusal: the gateway's
/// own error message when its envelope carries one, else the status.
fn switch_refusal(refusal: &GatewayResponse) -> String {
    value_from_bytes(&refusal.body)
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || format!("gateway declined the switch with status {}", refusal.status),
            str::to_string,
        )
}

/// Posts one streaming chat completion to the gateway and returns the
/// chat in `Opening` state, its tape guard already armed. Nothing is
/// awaited here: the returned chat's open future resolves in the session
/// loop's merged branch, where [`advance_chat`] either transitions it to
/// `Streaming` or settles it - an `error` frame, plus a tape event where
/// an exchange happened. Arming the guard before the gateway answers is
/// what gives a chat canceled or abandoned while still queued its one
/// tape event.
fn begin_chat(
    state: &AppState,
    request: ChatRequest,
    frame: serde_json::Value,
    id: Option<serde_json::Value>,
) -> ActiveChat {
    let started = Instant::now();
    let push = state.push();
    push.push_status_update(
        "Submitting request...",
        format!("a streaming chat completion from {}", request.model),
        Activity::Thinking,
    );
    let tape = StreamTape::open(
        Arc::clone(state.tape()),
        request.model.clone(),
        frame,
        started,
        push,
    );
    let client = state.gateway_client().clone();
    let open = async move { client.chat_completion_stream(&request).await }.boxed();
    ActiveChat {
        work: ChatWork::Opening(open),
        id,
        tape,
    }
}

/// Whether one payload's frames all reached the client.
enum Forward {
    Sent,
    ClientGone,
}

/// Forwards one SSE payload's deltas to the client: the reasoning side
/// channel as a `reasoning` frame (for the UI's Thinking block, never part
/// of the taped response) and the answer content as a `delta` frame,
/// appended to the tape's assembled response. The chunk pulses at Debug:
/// the UI ignores the text, but the activity field keeps the LED lit.
async fn forward_payload(
    payload: &str,
    active: &mut ActiveChat,
    socket: &mut WebSocket,
    push: &Push,
    backoff: &ReconnectBackoff,
) -> Forward {
    let ActiveChat { id, tape, .. } = active;
    let fields = delta_fields(payload);
    // A delivered token is the useful work that resets the reconnect
    // backoff - the gateway proved it streams, not merely connects.
    // Whether the frame then reaches our own client says nothing about
    // the gateway, so the reset precedes the sends.
    if fields.content.is_some() || fields.reasoning.is_some() {
        backoff.record_useful_work();
    }
    if let Some(text) = fields.reasoning {
        push.push_activity(
            "Streaming response...",
            "a gateway reasoning chunk",
            Activity::Thinking,
        );
        if !send_frame(socket, &ReasoningFrame::new(text, id.as_ref())).await {
            return Forward::ClientGone;
        }
    }
    let Some(text) = fields.content else {
        return Forward::Sent;
    };
    tape.append(&text);
    push.push_activity(
        "Streaming response...",
        "a gateway response chunk",
        Activity::Generating,
    );
    if send_frame(socket, &DeltaFrame::new(text, id.as_ref())).await {
        Forward::Sent
    } else {
        Forward::ClientGone
    }
}

/// Handles a gateway that declined the stream with an ordinary response:
/// the envelope is taped like a buffered chat and reported as an `error`
/// frame and an error status.
async fn declined_stream(
    tape: StreamTape,
    upstream: GatewayResponse,
    id: Option<&serde_json::Value>,
    push: &Push,
    socket: &mut WebSocket,
) {
    let response = value_from_bytes(&upstream.body);
    tape.record_envelope(response.clone()).await;
    let message = response
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || {
                format!(
                    "gateway declined the stream with status {}",
                    upstream.status
                )
            },
            str::to_string,
        );
    push.push_failure(
        format!("Gateway error: {}", upstream.status),
        message.clone(),
        Activity::General,
    );
    send_error(socket, id, message).await;
}

/// The text fields of one streaming delta: answer content and the
/// reasoning side channel, either of which may be absent.
struct DeltaFields {
    content: Option<String>,
    reasoning: Option<String>,
}

/// Extracts the content and reasoning deltas from one gateway SSE payload.
///
/// Role-priming and usage events have no `choices[0].delta.content` and
/// contribute nothing to the assembled response. An empty-string content
/// delta (the common `{"role":"assistant","content":""}` priming chunk)
/// is filtered like an absent one: forwarding it would emit an empty
/// `delta` frame that closes the UI's Thinking block and flips the
/// activity LED before any answer text exists. Reasoning models stream
/// their scratch work under `reasoning_content` (or the `reasoning` /
/// `thinking` synonyms, matching promptforge-core's normalization); the
/// first non-empty synonym wins, so a present-but-empty key falls
/// through to a populated one instead of masking it.
fn delta_fields(payload: &str) -> DeltaFields {
    let empty = DeltaFields {
        content: None,
        reasoning: None,
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return empty;
    };
    let Some(delta) = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("delta"))
    else {
        return empty;
    };
    let content = delta
        .get("content")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let reasoning = ["reasoning_content", "reasoning", "thinking"]
        .iter()
        .filter_map(|key| delta.get(*key).and_then(serde_json::Value::as_str))
        .find(|text| !text.is_empty())
        .map(str::to_string);
    DeltaFields { content, reasoning }
}

/// Tape bookkeeping carried through one streaming chat, doubling as the
/// disconnect guard.
///
/// The settle paths consume it through [`StreamTape::record`], so a
/// streamed chat always tapes exactly one event; a chat abandoned
/// mid-stream drops it un-recorded, and the drop spawns the tape write
/// with the disconnect note and then returns the status bar to Ready -
/// the session loop's exit paths carry no cleanup calls.
struct StreamTape {
    /// Present until the chat settles; taken exactly once, by `record` or
    /// by the drop.
    entry: Option<TapeEntry>,
    push: Push,
}

/// What one tape event needs from the chat that produced it.
struct TapeEntry {
    tape: Arc<Tape>,
    model: String,
    request: serde_json::Value,
    started: Instant,
    /// Concatenation of every content delta forwarded so far.
    assembled: String,
}

impl StreamTape {
    /// Arms the guard for one streaming chat.
    fn open(
        tape: Arc<Tape>,
        model: String,
        request: serde_json::Value,
        started: Instant,
        push: Push,
    ) -> Self {
        Self {
            entry: Some(TapeEntry {
                tape,
                model,
                request,
                started,
                assembled: String::new(),
            }),
            push,
        }
    }

    /// Appends one forwarded content delta to the assembled response.
    fn append(&mut self, text: &str) {
        if let Some(entry) = self.entry.as_mut() {
            entry.assembled.push_str(text);
        }
    }

    /// Writes the stream's single tape event: the assembled content on
    /// success, or `error` beside the partial content on failure.
    async fn record(mut self, error: Option<String>) {
        if let Some(entry) = self.entry.take() {
            entry.write(error).await;
        }
    }

    /// Writes the declined-stream tape event: the gateway's buffered
    /// error envelope verbatim, in place of an assembled response.
    async fn record_envelope(mut self, response: serde_json::Value) {
        if let Some(entry) = self.entry.take() {
            let TapeEntry {
                tape,
                model,
                request,
                started,
                ..
            } = entry;
            tape_round_trip(&tape, model, request, response, started.elapsed()).await;
        }
    }

    /// Disarms the guard without taping: the open failed before any
    /// response arrived, so no exchange happened and nothing is taped -
    /// matching the heartbeat short-circuit's no-tape rule.
    fn discard(mut self) {
        self.entry = None;
    }
}

impl Drop for StreamTape {
    fn drop(&mut self) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        let push = self.push.clone();
        // Drop cannot await, so the abandoned exchange is taped from a
        // spawned task; the idle push follows the write inside that task,
        // so a status observer that sees Ready can trust the tape to hold
        // the disconnect note.
        tokio::spawn(async move {
            entry
                .write(Some("client disconnected mid-stream".to_string()))
                .await;
            push.push_idle();
        });
    }
}

impl TapeEntry {
    /// Writes the tape event this entry was collected for.
    async fn write(self, error: Option<String>) {
        let Self {
            tape,
            model,
            request,
            started,
            assembled,
        } = self;
        let response = match error {
            Some(message) => serde_json::json!({
                "error": message,
                "content": assembled,
            }),
            None => serde_json::Value::String(assembled),
        };
        tape_round_trip(&tape, model, request, response, started.elapsed()).await;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use futures_util::{SinkExt, stream};
    use tokio_tungstenite::tungstenite;

    use crate::app::fixtures::{spawn_gateway, state_for};
    use crate::app::router;
    use crate::protocol::{Activity, Progress, Severity, StatusBarUpdate};

    const STREAM_BODY: &str = concat!(
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"po\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ng\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    const UPSTREAM_ERROR: &str =
        r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;

    /// A reasoning model's stream: scratch work on the side channel first,
    /// then the answer content.
    const REASONING_STREAM_BODY: &str = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hmm \"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"okay\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"po\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ng\"}}]}\n\n",
        "data: [DONE]\n\n",
    );

    #[test]
    fn an_empty_reasoning_synonym_falls_through_to_a_populated_one() {
        let fields = delta_fields(
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"","reasoning":"actual scratch work"}}]}"#,
        );
        assert_eq!(fields.reasoning.as_deref(), Some("actual scratch work"));
        assert_eq!(fields.content, None);
    }

    #[test]
    fn all_empty_reasoning_synonyms_yield_no_reasoning() {
        let fields = delta_fields(
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"","reasoning":"","thinking":"","content":"answer"}}]}"#,
        );
        assert!(fields.reasoning.is_none());
        assert_eq!(fields.content.as_deref(), Some("answer"));
    }

    #[test]
    fn an_empty_content_delta_is_filtered_like_an_absent_one() {
        // The common role-priming chunk carries `content: ""`; forwarding
        // it would emit an empty delta frame that closes the UI's Thinking
        // block and flips the activity LED before any answer text exists.
        let fields =
            delta_fields(r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#);
        assert_eq!(fields.content, None);
        assert!(fields.reasoning.is_none());
        // An empty content beside live reasoning must not mask the
        // reasoning nor emit an answer frame mid-think.
        let fields = delta_fields(
            r#"{"choices":[{"index":0,"delta":{"content":"","reasoning_content":"scratch"}}]}"#,
        );
        assert_eq!(fields.content, None);
        assert_eq!(fields.reasoning.as_deref(), Some("scratch"));
    }

    fn authorized(headers: &HeaderMap) -> bool {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some("Bearer test-key")
    }

    async fn mock_chat_stream(headers: HeaderMap, body: String) -> Response {
        assert!(authorized(&headers));
        let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
        assert_eq!(body["stream"], true, "the stream flag is forwarded");
        ([(header::CONTENT_TYPE, "text/event-stream")], STREAM_BODY).into_response()
    }

    /// Answers with one good SSE event, then aborts the body mid-stream.
    ///
    /// The pause after the first chunk gives hyper time to flush the headers
    /// and the event before the body errors, so the client observes a stream
    /// that fails mid-way rather than a connection that never answered.
    async fn mock_chat_stream_dies(headers: HeaderMap, body: String) -> Response {
        assert!(authorized(&headers));
        let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
        assert_eq!(body["stream"], true, "the stream flag is forwarded");
        let chunks = stream::unfold(0u8, |step| async move {
            match step {
                0 => Some((
                    Ok::<_, std::io::Error>(axum::body::Bytes::from_static(
                        b"data: {\"choices\":[{\"delta\":{\"content\":\"po\"}}]}\n\n",
                    )),
                    1,
                )),
                1 => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    Some((Err(std::io::Error::other("injected upstream failure")), 2))
                }
                _ => None,
            }
        });
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(chunks),
        )
            .into_response()
    }

    /// Drips one delta every 50ms, giving a client time to disconnect
    /// mid-stream before the drip runs out.
    async fn mock_chat_stream_drips(headers: HeaderMap, body: String) -> Response {
        assert!(authorized(&headers));
        let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
        assert_eq!(body["stream"], true, "the stream flag is forwarded");
        let chunks = stream::unfold(0u8, |step| async move {
            if step >= 40 {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let payload =
                format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"x{step}\"}}}}]}}\n\n");
            Some((
                Ok::<_, std::io::Error>(axum::body::Bytes::from(payload)),
                step + 1,
            ))
        });
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(chunks),
        )
            .into_response()
    }

    /// Declines a streaming request with an ordinary JSON error envelope.
    async fn mock_chat_declines_stream(headers: HeaderMap, body: String) -> Response {
        assert!(authorized(&headers));
        let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
        assert_eq!(body["stream"], true, "the stream flag is forwarded");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            UPSTREAM_ERROR,
        )
            .into_response()
    }

    /// Binds the workshop router against the gateway at `base_url` on a
    /// free loopback port and returns the `/ws` URL, the tempdir keeping
    /// the tape alive, and a handle on the shared state (for poking the
    /// status and catalog buses directly).
    async fn spawn_chat_server(base_url: &str) -> (String, tempfile::TempDir, AppState) {
        let (state, tape_dir) = state_for(base_url);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the chat test server");
        let addr = listener.local_addr().expect("chat test server address");
        let served = state.clone();
        tokio::spawn(async move {
            axum::serve(listener, router(served))
                .await
                .expect("chat test server serves");
        });
        (format!("ws://{addr}/ws"), tape_dir, state)
    }

    /// Reads one text frame from the client socket and parses it as JSON.
    async fn read_frame<S>(socket: &mut S) -> serde_json::Value
    where
        S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
    {
        let message = socket
            .next()
            .await
            .expect("a frame follows")
            .expect("the frame is not a socket error");
        let text = message.into_text().expect("the frame is text");
        serde_json::from_str(&text).expect("the frame is JSON")
    }

    /// Reads frames until one arrives that is not a status update. Status
    /// frames are unsolicited - the snapshot on connect, then bus pushes
    /// that may interleave with a chat's replies at any point - so reply
    /// assertions skip them.
    async fn read_non_status_frame<S>(socket: &mut S) -> serde_json::Value
    where
        S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
    {
        loop {
            let frame = read_frame(socket).await;
            if frame["type"] != "status" {
                return frame;
            }
        }
    }

    /// Sends one well-formed chat frame naming the test model.
    async fn send_chat<S>(socket: &mut S)
    where
        S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    {
        let frame = serde_json::json!({
            "type": "chat",
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        })
        .to_string();
        socket
            .send(tungstenite::Message::Text(frame.into()))
            .await
            .expect("the chat frame is sent");
    }

    /// Reads every event on the test's tape.
    fn tape_events(tape_dir: &tempfile::TempDir) -> Vec<serde_json::Value> {
        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        raw.lines()
            .map(|line| serde_json::from_str(line).expect("the tape line is valid JSON"))
            .collect()
    }

    #[tokio::test]
    async fn a_delivered_token_resets_the_backoff() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (url, _tape_dir, state) = spawn_chat_server(&base_url).await;
        // The backoff stands escalated, as after an outage; the first
        // streamed token is the useful work that returns it to base.
        let _ = state.backoff().next_delay();
        let _ = state.backoff().next_delay();
        assert!(state.backoff().is_escalated_for_test());
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(first, serde_json::json!({"type": "delta", "content": "po"}));
        assert!(
            !state.backoff().is_escalated_for_test(),
            "a delivered token is useful work and resets the backoff"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_delivered_reasoning_token_resets_the_backoff() {
        let base_url = spawn_gateway(
            Router::new().route("/v1/chat/completions", post(mock_chat_stream_reasons)),
        )
        .await;
        let (url, _tape_dir, state) = spawn_chat_server(&base_url).await;
        // The reasoning side channel streams before any answer content,
        // so a reset observed on its first chunk proves a reasoning
        // token counts as useful work on its own.
        let _ = state.backoff().next_delay();
        let _ = state.backoff().next_delay();
        assert!(state.backoff().is_escalated_for_test());
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(
            first,
            serde_json::json!({"type": "reasoning", "content": "hmm "})
        );
        assert!(
            !state.backoff().is_escalated_for_test(),
            "a streamed reasoning token is useful work and resets the backoff"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn chat_frames_relay_deltas_in_order_then_done() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (url, _tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        // The role-priming event carries no content and yields no frame.
        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(first, serde_json::json!({"type": "delta", "content": "po"}));
        let second = read_non_status_frame(&mut socket).await;
        assert_eq!(
            second,
            serde_json::json!({"type": "delta", "content": "ng"})
        );
        let third = read_non_status_frame(&mut socket).await;
        assert_eq!(third, serde_json::json!({"type": "done"}));
        socket.close(None).await.expect("close the socket");
    }

    /// Streams `REASONING_STREAM_BODY` as a mock reasoning model.
    async fn mock_chat_stream_reasons(headers: HeaderMap, body: String) -> Response {
        assert!(authorized(&headers));
        let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
        assert_eq!(body["stream"], true, "the stream flag is forwarded");
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            REASONING_STREAM_BODY,
        )
            .into_response()
    }

    #[tokio::test]
    async fn reasoning_deltas_relay_as_reasoning_frames_and_stay_off_the_tape() {
        let base_url = spawn_gateway(
            Router::new().route("/v1/chat/completions", post(mock_chat_stream_reasons)),
        )
        .await;
        let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(
            first,
            serde_json::json!({"type": "reasoning", "content": "hmm "}),
            "the reasoning side channel arrives as reasoning frames"
        );
        let second = read_non_status_frame(&mut socket).await;
        assert_eq!(
            second,
            serde_json::json!({"type": "reasoning", "content": "okay"})
        );
        let third = read_non_status_frame(&mut socket).await;
        assert_eq!(third, serde_json::json!({"type": "delta", "content": "po"}));
        let fourth = read_non_status_frame(&mut socket).await;
        assert_eq!(
            fourth,
            serde_json::json!({"type": "delta", "content": "ng"})
        );
        let fifth = read_non_status_frame(&mut socket).await;
        assert_eq!(fifth, serde_json::json!({"type": "done"}));

        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 1, "exactly one event per chat frame");
        assert_eq!(
            events[0]["response"], "pong",
            "the tape holds the answer content only, never the reasoning"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_completed_chat_tapes_one_event_with_the_assembled_response() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;
        // The terminal frame is sent after the tape write, so holding `done`
        // means the tape is durable.
        loop {
            let frame = read_non_status_frame(&mut socket).await;
            if frame["type"] == "done" {
                break;
            }
        }

        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 1, "exactly one event per chat frame");
        let event = &events[0];
        assert_eq!(event["kind"], "chat");
        assert_eq!(event["model"], "test-model");
        assert_eq!(
            event["request"]["type"], "chat",
            "the frame is taped as received"
        );
        assert_eq!(event["request"]["messages"][0]["content"], "ping");
        assert_eq!(
            event["response"], "pong",
            "the tape holds the assembled content, not the raw frames"
        );
        assert!(event["latency_ms"].is_u64(), "latency_ms is an integer");
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_mid_stream_gateway_error_sends_an_error_frame_and_tapes_the_note() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream_dies)))
                .await;
        let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(first, serde_json::json!({"type": "delta", "content": "po"}));
        let second = read_non_status_frame(&mut socket).await;
        assert_eq!(second["type"], "error");
        let message = second["message"].as_str().expect("the error is a string");
        assert!(!message.is_empty(), "the error frame names the failure");

        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 1, "an errored stream still tapes one event");
        let note = events[0]["response"]["error"]
            .as_str()
            .expect("the error note is a string");
        assert!(!note.is_empty(), "the error note names the failure");
        assert_eq!(
            events[0]["response"]["content"], "po",
            "the partial content is taped alongside the error"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_client_disconnect_mid_stream_is_taped_with_the_partial_content() {
        let base_url = spawn_gateway(
            Router::new().route("/v1/chat/completions", post(mock_chat_stream_drips)),
        )
        .await;
        let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
        // A second session observes the status bus: the dead client's
        // terminal update must still reach every remaining subscriber.
        let (mut observer, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect an observer to /ws");
        // Every session first receives the retained status snapshot -
        // Ready here, since the state is idle. Consume it so the Ready
        // watched for below can only be the post-disconnect idle.
        let snapshot = read_frame(&mut observer).await;
        assert_eq!(snapshot["type"], "status", "the snapshot arrives first");
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;
        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(first["type"], "delta");
        // Drop the socket without a close handshake; the server notices when
        // a later delta send fails.
        drop(socket);

        // The observer sees the relay return to Ready once the failed send
        // ends the stream, rather than keeping a stale activity LED.
        let idle = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let frame = read_frame(&mut observer).await;
                if frame["type"] == "status" && frame["label"] == "Ready" {
                    break frame;
                }
            }
        })
        .await
        .expect("the observer sees the idle status after the disconnect");
        assert_eq!(idle["activity"], "general");
        assert_eq!(idle["severity"], "info");

        // The tape write follows the failed send, so poll for it.
        let mut events: Vec<serde_json::Value> = Vec::new();
        for _ in 0..100 {
            if let Ok(raw) = std::fs::read_to_string(tape_dir.path().join("tape.jsonl"))
                && !raw.trim().is_empty()
            {
                events = raw
                    .lines()
                    .map(|line| serde_json::from_str(line).expect("the tape line is valid JSON"))
                    .collect();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(events.len(), 1, "a mid-stream disconnect tapes one event");
        assert_eq!(
            events[0]["response"]["error"], "client disconnected mid-stream",
            "the disconnect is taped as an error note"
        );
        let partial = events[0]["response"]["content"]
            .as_str()
            .expect("the partial content is a string");
        assert!(
            partial.starts_with("x0"),
            "the partial content is taped alongside: {partial:?}"
        );
    }

    #[tokio::test]
    async fn a_declined_stream_sends_an_error_frame_and_tapes_the_envelope() {
        let base_url = spawn_gateway(
            Router::new().route("/v1/chat/completions", post(mock_chat_declines_stream)),
        )
        .await;
        let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        let frame = read_non_status_frame(&mut socket).await;
        assert_eq!(frame["type"], "error");
        assert_eq!(frame["message"], "model unloaded");

        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 1, "a declined stream tapes exactly one event");
        assert_eq!(
            events[0]["response"]["error"]["code"], "upstream_unavailable",
            "the gateway's own envelope is taped"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn malformed_frames_are_answered_with_error_frames() {
        let (url, _tape_dir, _state) = spawn_chat_server("http://127.0.0.1:1").await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");

        for bad in [
            "not json",
            r#"{"type":"bogus"}"#,
            r#"{"type":"chat","model":"test-model"}"#,
        ] {
            socket
                .send(tungstenite::Message::Text(bad.into()))
                .await
                .expect("the frame is sent");
            let frame = read_non_status_frame(&mut socket).await;
            assert_eq!(
                frame["type"], "error",
                "a malformed frame is answered, not fatal: {bad}"
            );
        }
        // The session survives: a well-formed frame still gets through to
        // the (unreachable) gateway and answers with its own error.
        send_chat(&mut socket).await;
        let frame = read_non_status_frame(&mut socket).await;
        assert_eq!(frame["type"], "error");
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn status_updates_reach_connected_sessions_as_status_frames() {
        let (url, _tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        // A malformed frame's error reply proves the session loop is
        // running (the connect snapshot rides ahead of it and is skipped).
        socket
            .send(tungstenite::Message::Text("not json".into()))
            .await
            .expect("the frame is sent");
        let reply = read_non_status_frame(&mut socket).await;
        assert_eq!(reply["type"], "error");

        state.status().emit(StatusBarUpdate {
            label: "Downloading model".to_string(),
            description: "ggml-large-v3.bin".to_string(),
            progress: Some(Progress {
                current: 1,
                total: 2,
            }),
            severity: Severity::Info,
            activity: Activity::Generating,
        });

        let frame = read_frame(&mut socket).await;
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "status",
                "label": "Downloading model",
                "description": "ggml-large-v3.bin",
                "progress": {"current": 1, "total": 2},
                "severity": "info",
                "activity": "generating",
            }),
            "the update arrives as one status frame"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_new_session_receives_the_retained_status_and_catalog_snapshots() {
        let (url, _tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
        // Both pushes land on the buses while nobody is connected, so only
        // the retained copies can deliver them to the socket below - the
        // contract's resend-on-reconnect for ephemeral frames.
        state.status().emit(StatusBarUpdate {
            label: "Downloading model".to_string(),
            description: "ggml-large-v3.bin".to_string(),
            progress: Some(Progress {
                current: 1,
                total: 2,
            }),
            severity: Severity::Info,
            activity: Activity::Generating,
        });
        state.catalog().publish(vec![
            serde_json::json!({"id": "test-model", "object": "model"}),
        ]);

        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        let first = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
            .await
            .expect("the status snapshot arrives unprompted");
        assert_eq!(
            first,
            serde_json::json!({
                "type": "status",
                "label": "Downloading model",
                "description": "ggml-large-v3.bin",
                "progress": {"current": 1, "total": 2},
                "severity": "info",
                "activity": "generating",
            }),
            "the retained status is the connection's first frame"
        );
        let second = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
            .await
            .expect("the catalog snapshot arrives unprompted");
        assert_eq!(
            second,
            serde_json::json!({
                "type": "models",
                "models": [{"id": "test-model", "object": "model"}],
            }),
            "the retained catalog follows it"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn sequential_chats_on_one_socket_both_complete() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");

        for round in 1..=2 {
            let frame = serde_json::json!({
                "type": "chat",
                "id": round,
                "model": "test-model",
                "messages": [{"role": "user", "content": "ping"}],
            })
            .to_string();
            socket
                .send(tungstenite::Message::Text(frame.into()))
                .await
                .expect("the chat frame is sent");
            let first = read_non_status_frame(&mut socket).await;
            assert_eq!(
                first,
                serde_json::json!({"type": "delta", "content": "po", "id": round}),
                "round {round}: the first delta carries the request id"
            );
            let second = read_non_status_frame(&mut socket).await;
            assert_eq!(
                second,
                serde_json::json!({"type": "delta", "content": "ng", "id": round})
            );
            let third = read_non_status_frame(&mut socket).await;
            assert_eq!(third, serde_json::json!({"type": "done", "id": round}));
        }

        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 2, "one tape event per chat frame");
        assert!(
            events.iter().all(|event| event["response"] == "pong"),
            "both rounds taped the assembled response"
        );
        socket.close(None).await.expect("close the socket");
    }

    /// Drips deltas whose content names the request's arrival order -
    /// "c0-0", "c0-1", ... for the first request - one every 25 ms. The
    /// first request drips eight chunks and every later one four, so two
    /// overlapping chats always settle later-first: the first chat sent
    /// outlives the second.
    async fn mock_chat_stream_drips_indexed(
        State(counter): State<Arc<AtomicU64>>,
        headers: HeaderMap,
        body: String,
    ) -> Response {
        assert!(authorized(&headers));
        let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
        assert_eq!(body["stream"], true, "the stream flag is forwarded");
        let request = counter.fetch_add(1, Ordering::Relaxed);
        let chunks = if request == 0 { 8u64 } else { 4u64 };
        let drip = stream::unfold(0u64, move |step| async move {
            if step >= chunks {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
            let payload = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"c{request}-{step}\"}}}}]}}\n\n"
            );
            Some((
                Ok::<_, std::io::Error>(axum::body::Bytes::from(payload)),
                step + 1,
            ))
        });
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            Body::from_stream(drip),
        )
            .into_response()
    }

    /// Spawns the chat server against a gateway dripping indexed deltas
    /// (first request long, later ones short).
    async fn spawn_indexed_drip_server() -> (String, tempfile::TempDir, AppState) {
        let base_url = spawn_gateway(
            Router::new()
                .route("/v1/chat/completions", post(mock_chat_stream_drips_indexed))
                .with_state(Arc::new(AtomicU64::new(0))),
        )
        .await;
        spawn_chat_server(&base_url).await
    }

    /// Sends one well-formed chat frame naming the test model, tagged
    /// with `id`.
    async fn send_tagged_chat<S>(socket: &mut S, id: u64)
    where
        S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    {
        let frame = serde_json::json!({
            "type": "chat",
            "id": id,
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        })
        .to_string();
        socket
            .send(tungstenite::Message::Text(frame.into()))
            .await
            .expect("the chat frame is sent");
    }

    /// Sends one cancel frame naming `id`.
    async fn send_cancel<S>(socket: &mut S, id: u64)
    where
        S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    {
        let frame = serde_json::json!({"type": "cancel", "id": id}).to_string();
        socket
            .send(tungstenite::Message::Text(frame.into()))
            .await
            .expect("the cancel frame is sent");
    }

    /// Reads non-status frames until `expected` terminal `done` frames
    /// have arrived, returning every frame read, in order.
    async fn replies_until_dones<S>(socket: &mut S, expected: usize) -> Vec<serde_json::Value>
    where
        S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
    {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut replies = Vec::new();
            let mut settled = 0;
            while settled < expected {
                let frame = read_non_status_frame(socket).await;
                if frame["type"] == "done" {
                    settled += 1;
                }
                replies.push(frame);
            }
            replies
        })
        .await
        .expect("every chat settles within the deadline")
    }

    #[tokio::test]
    async fn two_concurrent_chats_interleave_deltas_and_tape_one_event_each() {
        let (url, tape_dir, _state) = spawn_indexed_drip_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_tagged_chat(&mut socket, 1).await;
        send_tagged_chat(&mut socket, 2).await;
        let replies = replies_until_dones(&mut socket, 2).await;

        // Chat 1 reached the gateway first, so it got the mock's longer
        // first-request stream and outlives chat 2.
        let first_done = replies
            .iter()
            .position(|frame| frame["type"] == "done")
            .expect("a done frame arrived");
        assert_eq!(
            replies[first_done]["id"], 2,
            "the shorter chat settles first: {replies:?}"
        );
        assert!(
            replies[..first_done]
                .iter()
                .any(|frame| frame["type"] == "delta" && frame["id"] == 1),
            "chat 1's deltas arrive while chat 2 streams: {replies:?}"
        );
        assert!(
            replies[first_done..]
                .iter()
                .any(|frame| frame["type"] == "delta" && frame["id"] == 1),
            "chat 1 keeps streaming after chat 2 settles: {replies:?}"
        );
        for frame in replies.iter().filter(|frame| frame["type"] == "delta") {
            let id = frame["id"].as_u64().expect("every delta carries its id");
            let prefix = if id == 1 { "c0-" } else { "c1-" };
            assert!(
                frame["content"]
                    .as_str()
                    .expect("delta content is text")
                    .starts_with(prefix),
                "chat {id} carries its own stream's content: {frame}"
            );
        }

        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 2, "one tape event per chat");
        let responses: Vec<&str> = events
            .iter()
            .map(|event| event["response"].as_str().expect("a taped assembly"))
            .collect();
        assert!(
            responses.contains(&"c0-0c0-1c0-2c0-3c0-4c0-5c0-6c0-7"),
            "chat 1 taped its full assembly: {responses:?}"
        );
        assert!(
            responses.contains(&"c1-0c1-1c1-2c1-3"),
            "chat 2 taped its full assembly: {responses:?}"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn per_chat_frame_order_holds_while_chats_interleave() {
        let (url, _tape_dir, _state) = spawn_indexed_drip_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_tagged_chat(&mut socket, 1).await;
        send_tagged_chat(&mut socket, 2).await;
        let replies = replies_until_dones(&mut socket, 2).await;

        for (id, prefix, count) in [(1, "c0-", 8), (2, "c1-", 4)] {
            let chat: Vec<&serde_json::Value> =
                replies.iter().filter(|frame| frame["id"] == id).collect();
            let (terminal, deltas) = chat.split_last().expect("the chat produced frames");
            assert_eq!(
                terminal["type"], "done",
                "chat {id}'s terminal follows every delta"
            );
            let contents: Vec<&str> = deltas
                .iter()
                .map(|frame| {
                    assert_eq!(frame["type"], "delta");
                    frame["content"].as_str().expect("delta content is text")
                })
                .collect();
            let expected: Vec<String> = (0..count).map(|step| format!("{prefix}{step}")).collect();
            assert_eq!(
                contents, expected,
                "chat {id}'s deltas arrive in stream order despite the interleave"
            );
        }
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_second_untagged_chat_is_refused_while_one_streams() {
        let (url, tape_dir, _state) = spawn_indexed_drip_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;
        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(first["type"], "delta", "the first chat streams");

        send_chat(&mut socket).await;
        // The refusal interleaves with the first chat's deltas.
        let refusal = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = read_non_status_frame(&mut socket).await;
                if frame["type"] == "error" {
                    break frame;
                }
                assert_eq!(frame["type"], "delta", "the first chat is untouched");
            }
        })
        .await
        .expect("the refusal arrives while the first chat streams");
        assert!(
            refusal["message"]
                .as_str()
                .expect("the refusal names the rule")
                .contains("untagged"),
            "the refusal names the untagged rule: {refusal}"
        );
        assert!(
            refusal.get("id").is_none(),
            "the refused chat had no id to echo"
        );

        // The first chat still streams to completion and tapes its event;
        // the refused one never opened, so it tapes nothing.
        replies_until_dones(&mut socket, 1).await;
        assert_eq!(tape_events(&tape_dir).len(), 1, "only the live chat taped");
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_chat_reusing_a_live_id_is_refused() {
        let (url, tape_dir, _state) = spawn_indexed_drip_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_tagged_chat(&mut socket, 7).await;
        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(first["type"], "delta", "the first chat streams");

        send_tagged_chat(&mut socket, 7).await;
        let refusal = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = read_non_status_frame(&mut socket).await;
                if frame["type"] == "error" {
                    break frame;
                }
                assert_eq!(frame["type"], "delta", "the first chat is untouched");
            }
        })
        .await
        .expect("the refusal arrives while the first chat streams");
        assert_eq!(refusal["id"], 7, "the refusal echoes the duplicate id");
        assert!(
            refusal["message"]
                .as_str()
                .expect("the refusal names the rule")
                .contains("already streaming"),
            "the refusal names the duplicate-id rule: {refusal}"
        );

        replies_until_dones(&mut socket, 1).await;
        assert_eq!(tape_events(&tape_dir).len(), 1, "only the live chat taped");
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_mid_stream_disconnect_tapes_every_in_flight_chats_note() {
        // The long drip on both requests keeps both chats mid-stream well
        // past the moment the failed send surfaces the disconnect.
        let base_url = spawn_gateway(
            Router::new().route("/v1/chat/completions", post(mock_chat_stream_drips)),
        )
        .await;
        let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_tagged_chat(&mut socket, 1).await;
        send_tagged_chat(&mut socket, 2).await;
        // One delta from each chat proves both streams are live.
        tokio::time::timeout(Duration::from_secs(10), async {
            let (mut live_one, mut live_two) = (false, false);
            while !(live_one && live_two) {
                let frame = read_non_status_frame(&mut socket).await;
                assert_eq!(frame["type"], "delta");
                match frame["id"].as_u64() {
                    Some(1) => live_one = true,
                    Some(2) => live_two = true,
                    other => panic!("a delta of an unknown chat: {other:?}"),
                }
            }
        })
        .await
        .expect("both chats stream before the disconnect");
        // Drop the socket without a close handshake; the server notices
        // when a later delta send fails.
        drop(socket);

        // The tape writes follow the failed send, so poll for both.
        let mut events: Vec<serde_json::Value> = Vec::new();
        for _ in 0..100 {
            if let Ok(raw) = std::fs::read_to_string(tape_dir.path().join("tape.jsonl")) {
                events = raw
                    .lines()
                    .map(|line| serde_json::from_str(line).expect("the tape line is valid JSON"))
                    .collect();
                if events.len() == 2 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(events.len(), 2, "every in-flight chat tapes its own note");
        for event in &events {
            assert_eq!(
                event["response"]["error"], "client disconnected mid-stream",
                "each chat's abandonment is taped: {event}"
            );
        }
    }

    #[tokio::test]
    async fn a_cancel_ends_one_chat_while_the_other_streams_to_completion() {
        let (url, tape_dir, _state) = spawn_indexed_drip_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        // Chat 1 gets the long stream; chat 2's short stream outlives the
        // cancel below, so it settles after chat 1 is torn down.
        send_tagged_chat(&mut socket, 1).await;
        send_tagged_chat(&mut socket, 2).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let frame = read_non_status_frame(&mut socket).await;
                if frame["type"] == "delta" && frame["id"] == 1 {
                    break;
                }
            }
        })
        .await
        .expect("chat 1 streams before the cancel");
        send_cancel(&mut socket, 1).await;

        // Chat 2 streams to completion; chat 1 never settles on the wire.
        let replies = replies_until_dones(&mut socket, 1).await;
        let terminal = replies.last().expect("the done frame was collected");
        assert_eq!(
            terminal["id"], 2,
            "the surviving chat's terminal is the only done: {replies:?}"
        );

        // The canceled chat's tape write precedes the cancel frame's
        // handling returning, and chat 2's precedes its done, so both are
        // durable here.
        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 2, "both chats tape exactly one event each");
        let canceled = events
            .iter()
            .find(|event| event["response"]["error"] == "chat canceled by client")
            .expect("the canceled chat taped the abandonment");
        assert!(
            canceled["response"]["content"]
                .as_str()
                .expect("the partial content is a string")
                .starts_with("c0-"),
            "the partial content is taped beside the note: {canceled}"
        );
        assert!(
            events
                .iter()
                .any(|event| event["response"] == "c1-0c1-1c1-2c1-3"),
            "the surviving chat taped its full assembly: {events:?}"
        );
        socket.close(None).await.expect("close the socket");
    }

    /// A mock gateway whose admission is scripted by the request's user
    /// message: `"drip"` streams a long drip immediately, `"park"` holds
    /// the response headers - no bytes at all, the exact shape of a
    /// request waiting in a per-dominion queue at capacity - until the
    /// test fires the Notify, then streams `STREAM_BODY`.
    async fn mock_chat_stream_admission(
        State(gate): State<Arc<tokio::sync::Notify>>,
        headers: HeaderMap,
        body: String,
    ) -> Response {
        assert!(authorized(&headers));
        let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
        assert_eq!(body["stream"], true, "the stream flag is forwarded");
        if body["messages"][0]["content"] == "drip" {
            let chunks = stream::unfold(0u8, |step| async move {
                if step >= 40 {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                let payload = format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"x{step}\"}}}}]}}\n\n"
                );
                Some((
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(payload)),
                    step + 1,
                ))
            });
            return (
                [(header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(chunks),
            )
                .into_response();
        }
        if body["messages"][0]["content"] == "park" {
            gate.notified().await;
        }
        ([(header::CONTENT_TYPE, "text/event-stream")], STREAM_BODY).into_response()
    }

    /// Spawns the chat server against the scripted-admission gateway,
    /// returning the release handle for its parked requests.
    async fn spawn_admission_server() -> (String, tempfile::TempDir, Arc<tokio::sync::Notify>) {
        let gate = Arc::new(tokio::sync::Notify::new());
        let base_url = spawn_gateway(
            Router::new()
                .route("/v1/chat/completions", post(mock_chat_stream_admission))
                .with_state(Arc::clone(&gate)),
        )
        .await;
        let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
        (url, tape_dir, gate)
    }

    /// Sends one chat frame tagged with `id` whose user message is
    /// `content`, scripting the admission mock's behavior.
    async fn send_marked_chat<S>(socket: &mut S, id: u64, content: &str)
    where
        S: futures_util::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
    {
        let frame = serde_json::json!({
            "type": "chat",
            "id": id,
            "model": "test-model",
            "messages": [{"role": "user", "content": content}],
        })
        .to_string();
        socket
            .send(tungstenite::Message::Text(frame.into()))
            .await
            .expect("the chat frame is sent");
    }

    /// Polls the tape until it holds `expected` events, within a
    /// deadline, and returns them.
    async fn tape_events_when(
        tape_dir: &tempfile::TempDir,
        expected: usize,
    ) -> Vec<serde_json::Value> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(raw) = std::fs::read_to_string(tape_dir.path().join("tape.jsonl")) {
                    let events: Vec<serde_json::Value> = raw
                        .lines()
                        .map(|line| {
                            serde_json::from_str(line).expect("the tape line is valid JSON")
                        })
                        .collect();
                    if events.len() >= expected {
                        break events;
                    }
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("the tape holds the expected events within the deadline")
    }

    #[tokio::test]
    async fn a_chat_parked_at_gateway_admission_never_blocks_the_session() {
        let (url, tape_dir, gate) = spawn_admission_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_marked_chat(&mut socket, 1, "drip").await;
        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(first["id"], 1, "chat 1 streams before chat 2 is sent");

        // Chat 2 posts and parks: the mock holds its response headers,
        // exactly as the gateway's queue does at max_concurrency.
        send_marked_chat(&mut socket, 2, "park").await;
        // Chat 1's deltas keep flowing while chat 2 waits for admission:
        // the session loop did not block on the parked open.
        for _ in 0..3 {
            let frame =
                tokio::time::timeout(Duration::from_secs(10), read_non_status_frame(&mut socket))
                    .await
                    .expect("chat 1 keeps streaming while chat 2 is parked");
            assert_eq!(frame["type"], "delta");
            assert_eq!(frame["id"], 1, "only chat 1 streams while chat 2 is parked");
        }

        // A cancel for chat 1 - the one action that frees real capacity -
        // is read and processed while chat 2 is still parked: its tape
        // note lands without any release of the gate.
        send_cancel(&mut socket, 1).await;
        let events = tape_events_when(&tape_dir, 1).await;
        assert_eq!(
            events[0]["response"]["error"], "chat canceled by client",
            "the cancel settles while chat 2 waits for admission: {events:?}"
        );

        // Release chat 2's admission; it streams to completion.
        gate.notify_one();
        let replies = replies_until_dones(&mut socket, 1).await;
        let terminal = replies.last().expect("the done frame was collected");
        assert_eq!(
            terminal["id"], 2,
            "chat 2 settles once admitted: {replies:?}"
        );
        assert!(
            replies
                .iter()
                .any(|frame| frame["type"] == "delta" && frame["id"] == 2),
            "chat 2 streamed its deltas after release: {replies:?}"
        );
        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 2, "one tape event per chat");
        assert!(
            events.iter().any(|event| event["response"] == "pong"),
            "the released chat taped its full assembly: {events:?}"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_cancel_for_a_chat_still_opening_removes_it_cleanly() {
        let (url, tape_dir, _gate) = spawn_admission_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        // Chat 1 parks before its headers and is canceled there; the
        // gate is never released, so only the cancel can settle it.
        send_marked_chat(&mut socket, 1, "park").await;
        send_cancel(&mut socket, 1).await;

        // The cancel tapes the abandonment exactly once, with nothing
        // streamed yet.
        let events = tape_events_when(&tape_dir, 1).await;
        assert_eq!(events.len(), 1, "the canceled open tapes exactly once");
        assert_eq!(
            events[0]["response"]["error"], "chat canceled by client",
            "the abandonment is taped: {events:?}"
        );
        assert_eq!(
            events[0]["response"]["content"], "",
            "a chat canceled while opening streamed nothing"
        );

        // The canceled chat produces no frames afterward: a fresh chat
        // is admitted immediately (only "park" requests are held) and
        // every reply frame carries its id alone.
        send_marked_chat(&mut socket, 2, "ping").await;
        let replies = replies_until_dones(&mut socket, 1).await;
        assert!(
            replies.iter().all(|frame| frame["id"] == 2),
            "no frame of the canceled chat ever arrives: {replies:?}"
        );
        let events = tape_events(&tape_dir);
        assert_eq!(events.len(), 2, "the canceled chat's note stays single");
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_cancel_for_an_unknown_id_is_ignored() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (url, _tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_cancel(&mut socket, 99).await;
        // Replies are ordered, so if the cancel had drawn an error frame
        // it would arrive ahead of this chat's first delta.
        send_chat(&mut socket).await;
        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(
            first,
            serde_json::json!({"type": "delta", "content": "po"}),
            "the unknown cancel drew no reply and the session streams on"
        );
        replies_until_dones(&mut socket, 1).await;
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn idle_fires_only_after_the_last_in_flight_chat_settles() {
        let (url, _tape_dir, _state) = spawn_indexed_drip_server().await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        // Every session's first frame is the retained status snapshot,
        // which reads Ready on this idle fixture; consume it so a Ready
        // seen below can only be the settle path's idle push. No
        // heartbeat runs here, so no other producer pushes Ready.
        let snapshot = read_frame(&mut socket).await;
        assert_eq!(snapshot["type"], "status", "the snapshot arrives first");
        // Chat 2's shorter stream settles first; the idle push must wait
        // for chat 1.
        send_tagged_chat(&mut socket, 1).await;
        send_tagged_chat(&mut socket, 2).await;

        tokio::time::timeout(Duration::from_secs(30), async {
            let mut settled = 0;
            loop {
                let frame = read_frame(&mut socket).await;
                if frame["type"] == "done" {
                    settled += 1;
                    continue;
                }
                if frame["type"] == "status" && frame["label"] == "Ready" {
                    assert_eq!(
                        settled, 2,
                        "the idle push fired before the last chat settled"
                    );
                    break;
                }
            }
        })
        .await
        .expect("the idle push arrives once both chats settle");
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_gateway_known_down_short_circuits_chat_with_an_error_frame() {
        let (url, tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
        state.health().publish(false);
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        let frame = serde_json::json!({
            "type": "chat",
            "id": 7,
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        })
        .to_string();
        socket
            .send(tungstenite::Message::Text(frame.into()))
            .await
            .expect("the chat frame is sent");

        let reply = read_non_status_frame(&mut socket).await;
        assert_eq!(
            reply,
            serde_json::json!({"type": "error", "message": "Gateway unreachable", "id": 7}),
            "the chat fails fast, with the request id echoed"
        );
        socket.close(None).await.expect("close the socket");
        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        assert!(
            raw.trim().is_empty(),
            "no upstream attempt means no tape event"
        );
    }

    const CATALOG: &str = r#"{"object":"list","data":[{"id":"test-model","object":"model","owned_by":"promptforge"}]}"#;

    /// A mock `/health` whose answer flips under test control.
    async fn flippable_health(State(healthy): State<Arc<AtomicBool>>) -> Response {
        if healthy.load(Ordering::Relaxed) {
            StatusCode::OK.into_response()
        } else {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }

    /// A static mock catalog for the reconnect push test.
    async fn mock_models() -> Response {
        ([(header::CONTENT_TYPE, "application/json")], CATALOG).into_response()
    }

    // Un-ignored with the session rewrite: the flip below now waits for
    // the heartbeat's observed outage, so the recovery is always a real
    // down-to-up transition and the catalog push always happens; the
    // session's own subscription is live before the flip for the same
    // reason.
    #[tokio::test]
    async fn a_gateway_reconnect_pushes_the_refreshed_catalog_to_sessions() {
        let healthy = Arc::new(AtomicBool::new(false));
        let base_url = spawn_gateway(
            Router::new()
                .route("/health", get(flippable_health))
                .route("/v1/models", get(mock_models))
                .with_state(Arc::clone(&healthy)),
        )
        .await;
        let (url, _tape_dir, state) = spawn_chat_server(&base_url).await;
        let heartbeat = crate::heartbeat::spawn(
            state.gateway_client().clone(),
            state.push(),
            state.health().clone(),
            Duration::from_millis(25),
            // A fast schedule so the down-phase retry lands within a few
            // ticks instead of the production seconds.
            ReconnectBackoff::with_schedule(
                Duration::from_millis(10),
                Duration::from_millis(40),
                Duration::from_secs(60),
            ),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        // Hold the flip until the heartbeat's outage reaches this socket
        // (as the connect snapshot or a live push): the catalog is pushed
        // only on an observed down-to-up transition, so flipping before
        // the first probe lands would leave nothing to push.
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(30), read_frame(&mut socket))
                .await
                .expect("the heartbeat publishes the outage within the deadline");
            if frame["type"] == "status" && frame["label"] == "Gateway unreachable" {
                break;
            }
        }

        healthy.store(true, Ordering::Relaxed);
        // Status frames (the "Connected to gateway" transition) interleave
        // with the push; read until the models frame arrives.
        let frame = loop {
            let frame = tokio::time::timeout(Duration::from_secs(30), read_frame(&mut socket))
                .await
                .expect("frames keep arriving within the deadline");
            if frame["type"] == "models" {
                break frame;
            }
        };
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "models",
                "models": [{"id": "test-model", "object": "model", "owned_by": "promptforge"}],
            }),
            "the refreshed catalog arrives as one models frame"
        );
        socket.close(None).await.expect("close the socket");
        heartbeat.shutdown().await;
    }

    #[tokio::test]
    async fn a_chat_reports_submitting_then_streaming() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (url, _tape_dir, _state) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        let mut labels: Vec<String> = Vec::new();
        loop {
            let frame = read_frame(&mut socket).await;
            match frame["type"].as_str() {
                Some("status") => labels.push(
                    frame["label"]
                        .as_str()
                        .expect("a status frame carries a label")
                        .to_string(),
                ),
                Some("done") => break,
                _ => {}
            }
        }
        assert!(
            labels.iter().any(|label| label.contains("Submitting")),
            "a Submitting status frame arrived: {labels:?}"
        );
        assert!(
            labels.iter().any(|label| label.contains("Streaming")),
            "a Streaming status frame arrived: {labels:?}"
        );
        socket.close(None).await.expect("close the socket");
    }

    /// Reads frames until `accept` holds, within a generous deadline,
    /// returning every frame read - the accepted one last.
    async fn frames_until<S>(
        socket: &mut S,
        accept: impl Fn(&serde_json::Value) -> bool,
    ) -> Vec<serde_json::Value>
    where
        S: futures_util::Stream<Item = Result<tungstenite::Message, tungstenite::Error>> + Unpin,
    {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut frames = Vec::new();
            loop {
                let frame = read_frame(socket).await;
                let found = accept(&frame);
                frames.push(frame);
                if found {
                    return frames;
                }
            }
        })
        .await
        .expect("the expected frame arrives within the deadline")
    }

    #[tokio::test]
    async fn a_new_session_receives_the_retained_workbench_snapshot() {
        let (url, _tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
        // Both retained copies exist before anyone connects, so only the
        // connect-time sends can deliver them below - the boot-with-zero-
        // HTTP-fetches promise.
        state
            .catalog()
            .publish(vec![serde_json::json!({"id": "test-model"})]);
        state.menu().set_profiles(
            vec!["main".to_string(), "coding".to_string()],
            Some("main".to_string()),
        );

        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        let first = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
            .await
            .expect("the status snapshot arrives unprompted");
        assert_eq!(first["type"], "status", "the retained status arrives first");
        let second = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
            .await
            .expect("the catalog snapshot arrives unprompted");
        assert_eq!(second["type"], "models", "the retained catalog follows");
        let third = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut socket))
            .await
            .expect("the workbench snapshot arrives unprompted");
        assert_eq!(
            third,
            serde_json::json!({
                "type": "workbench",
                "profiles": ["main", "coding"],
                "active": "main",
                "switching": null,
                "selected": null,
                "chat_ready": false,
            }),
            "the retained workbench snapshot completes the boot state"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_select_model_event_round_trips_and_refusals_answer_errors() {
        let (url, _tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
        state
            .catalog()
            .publish(vec![serde_json::json!({"id": "test-model"})]);
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");

        let select = serde_json::json!({"type": "select_model", "model": "test-model"}).to_string();
        socket
            .send(tungstenite::Message::Text(select.into()))
            .await
            .expect("the select frame is sent");
        let frames = frames_until(&mut socket, |frame| frame["type"] == "workbench").await;
        let published = frames.last().expect("the accepted frame is last");
        assert_eq!(
            published["selected"], "test-model",
            "the selection round-trips as a workbench push"
        );

        let unknown =
            serde_json::json!({"type": "select_model", "model": "bogus", "id": 3}).to_string();
        socket
            .send(tungstenite::Message::Text(unknown.into()))
            .await
            .expect("the select frame is sent");
        let frames = frames_until(&mut socket, |frame| frame["type"] == "error").await;
        let refusal = frames.last().expect("the refusal is last");
        assert_eq!(refusal["id"], 3, "the refusal echoes the event id");
        assert!(
            refusal["message"]
                .as_str()
                .expect("the refusal names the rule")
                .contains("unknown model"),
            "the refusal names the unknown-model rule: {refusal}"
        );

        let missing = serde_json::json!({"type": "select_model"}).to_string();
        socket
            .send(tungstenite::Message::Text(missing.into()))
            .await
            .expect("the select frame is sent");
        let frames = frames_until(&mut socket, |frame| frame["type"] == "error").await;
        let refusal = frames.last().expect("the refusal is last");
        assert!(
            refusal["message"]
                .as_str()
                .expect("the refusal names the field")
                .contains("model"),
            "a field-less select is refused, not fatal: {refusal}"
        );
        socket.close(None).await.expect("close the socket");
    }

    /// Streams the full stage ladder then the terminal ready.
    async fn mock_switch_succeeds() -> Response {
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            concat!(
                "data: {\"stage\":\"loading-profile\"}\n\n",
                "data: {\"stage\":\"stopping-models\"}\n\n",
                "data: {\"stage\":\"starting-models\"}\n\n",
                "data: {\"status\":\"ready\",\"profile\":\"beta\"}\n\n",
            ),
        )
            .into_response()
    }

    /// Streams one stage then the terminal error.
    async fn mock_switch_fails() -> Response {
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            concat!(
                "data: {\"stage\":\"loading-profile\"}\n\n",
                "data: {\"status\":\"error\",\"message\":\"start-local failed\"}\n\n",
            ),
        )
            .into_response()
    }

    /// The profile endpoints the switch task's refetch hits.
    fn profile_routes(active: &'static str) -> Router {
        Router::new()
            .route(
                "/admin/profiles",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        r#"{"profiles":["main","beta"]}"#,
                    )
                }),
            )
            .route(
                "/admin/status",
                get(move || async move {
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        format!(r#"{{"profile":"{active}"}}"#),
                    )
                }),
            )
            .route("/v1/models", get(mock_models))
    }

    #[tokio::test]
    async fn a_switch_profile_event_streams_progress_and_settles_the_menu() {
        let base_url = spawn_gateway(
            Router::new()
                .route("/admin/switch-profile", post(mock_switch_succeeds))
                .merge(profile_routes("beta")),
        )
        .await;
        let (url, _tape_dir, state) = spawn_chat_server(&base_url).await;
        // Readiness needs a non-empty catalog and reachability; seed both
        // so the settled snapshot recomputes chat_ready to true. The seed
        // matches the refetched catalog so the reconcile stays a no-op.
        state.catalog().publish(vec![serde_json::json!(
            {"id": "test-model", "object": "model", "owned_by": "promptforge"}
        )]);
        state.menu().set_gateway_reachable(true);
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");

        let switch = serde_json::json!({"type": "switch_profile", "name": "beta"}).to_string();
        socket
            .send(tungstenite::Message::Text(switch.into()))
            .await
            .expect("the switch frame is sent");
        let frames = frames_until(&mut socket, |frame| {
            frame["type"] == "workbench"
                && frame["switching"].is_null()
                && frame["active"] == "beta"
        })
        .await;

        let pending = frames
            .iter()
            .find(|frame| frame["type"] == "workbench" && frame["switching"] == "beta")
            .expect("the pending snapshot was pushed before the settle");
        assert_eq!(
            pending["chat_ready"], false,
            "a switch in flight blocks chat: {pending}"
        );

        let progress: Vec<(String, u64, u64)> = frames
            .iter()
            .filter(|frame| frame["type"] == "status" && !frame["progress"].is_null())
            .map(|frame| {
                (
                    frame["label"]
                        .as_str()
                        .expect("a progress frame carries a label")
                        .to_string(),
                    frame["progress"]["current"]
                        .as_u64()
                        .expect("current is an integer"),
                    frame["progress"]["total"]
                        .as_u64()
                        .expect("total is an integer"),
                )
            })
            .collect();
        assert_eq!(
            progress,
            [
                ("Loading profile...".to_string(), 1, 3),
                ("Stopping models...".to_string(), 2, 3),
                ("Starting models...".to_string(), 3, 3),
            ],
            "each stage arrives as determinate progress, in execution order"
        );

        assert!(
            frames.iter().any(|frame| frame["type"] == "models"),
            "the refetched catalog arrives as a models frame: {frames:?}"
        );

        let settled = frames.last().expect("the settled snapshot is last");
        assert_eq!(
            settled["selected"], "test-model",
            "the settled snapshot selects the profile's model"
        );
        assert_eq!(
            settled["chat_ready"], true,
            "readiness recomputes once the switch settles"
        );
        assert_eq!(
            settled["profiles"],
            serde_json::json!(["main", "beta"]),
            "the refetched profile list rides into the snapshot"
        );

        // The idle push follows the settle; it may already have arrived
        // interleaved with the frames above.
        if !frames
            .iter()
            .any(|frame| frame["type"] == "status" && frame["label"] == "Ready")
        {
            frames_until(&mut socket, |frame| {
                frame["type"] == "status" && frame["label"] == "Ready"
            })
            .await;
        }
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_failed_switch_restores_the_menu_and_reports_the_failure() {
        let base_url = spawn_gateway(
            Router::new()
                .route("/admin/switch-profile", post(mock_switch_fails))
                // The gateway still serves the previous profile after the
                // failure, so its status endpoint keeps naming it.
                .merge(profile_routes("main")),
        )
        .await;
        let (url, _tape_dir, state) = spawn_chat_server(&base_url).await;
        state.catalog().publish(vec![serde_json::json!(
            {"id": "test-model", "object": "model", "owned_by": "promptforge"}
        )]);
        state.menu().set_gateway_reachable(true);
        state.menu().set_profiles(
            vec!["main".to_string(), "beta".to_string()],
            Some("main".to_string()),
        );
        state
            .menu()
            .set_selected("test-model")
            .expect("the id is in the catalog");
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");

        let switch = serde_json::json!({"type": "switch_profile", "name": "beta"}).to_string();
        socket
            .send(tungstenite::Message::Text(switch.into()))
            .await
            .expect("the switch frame is sent");
        frames_until(&mut socket, |frame| {
            frame["type"] == "workbench" && frame["switching"] == "beta"
        })
        .await;
        let mut frames = frames_until(&mut socket, |frame| {
            frame["type"] == "workbench" && frame["switching"].is_null()
        })
        .await;

        let restored = frames.last().expect("the restored snapshot is last");
        assert_eq!(
            restored["active"], "main",
            "the previous profile still serves: {restored}"
        );
        assert_eq!(
            restored["selected"], "test-model",
            "the selection survives the failed switch"
        );
        assert_eq!(
            restored["chat_ready"], true,
            "readiness returns to its truthful pre-switch state"
        );

        // The failure status and the restored snapshot ride different
        // buses, so their wire order is not pinned; read on if needed.
        let is_failure =
            |frame: &serde_json::Value| frame["type"] == "status" && frame["severity"] == "error";
        if !frames.iter().any(is_failure) {
            frames.extend(frames_until(&mut socket, is_failure).await);
        }
        let failure = frames
            .iter()
            .find(|frame| is_failure(frame))
            .expect("the failure status was pushed");
        assert_eq!(failure["label"], "Profile switch failed");
        assert_eq!(
            failure["description"], "start-local failed",
            "the gateway's own message is reported"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_switch_while_one_runs_is_refused_with_an_error_frame() {
        let (url, _tape_dir, state) = spawn_chat_server("http://127.0.0.1:1").await;
        state
            .menu()
            .begin_switch("running")
            .expect("no switch is in flight");
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");

        let switch =
            serde_json::json!({"type": "switch_profile", "name": "beta", "id": 9}).to_string();
        socket
            .send(tungstenite::Message::Text(switch.into()))
            .await
            .expect("the switch frame is sent");
        let frames = frames_until(&mut socket, |frame| frame["type"] == "error").await;
        let refusal = frames.last().expect("the refusal is last");
        assert_eq!(refusal["id"], 9, "the refusal echoes the event id");
        assert!(
            refusal["message"]
                .as_str()
                .expect("the refusal names the rule")
                .contains("already in progress"),
            "the refusal names the single-flight rule: {refusal}"
        );
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_menu_event_lands_and_answers_while_a_chat_streams() {
        let (url, _tape_dir, state) = spawn_indexed_drip_server().await;
        state
            .catalog()
            .publish(vec![serde_json::json!({"id": "test-model"})]);
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_tagged_chat(&mut socket, 1).await;
        frames_until(&mut socket, |frame| frame["type"] == "delta").await;

        let select = serde_json::json!({"type": "select_model", "model": "test-model"}).to_string();
        socket
            .send(tungstenite::Message::Text(select.into()))
            .await
            .expect("the select frame is sent");
        let frames = frames_until(&mut socket, |frame| frame["type"] == "workbench").await;
        assert!(
            frames.iter().all(|frame| frame["type"] != "done"),
            "the selection answered while the chat still streamed: {frames:?}"
        );
        assert_eq!(
            frames.last().expect("the workbench push is last")["selected"],
            "test-model"
        );

        // The chat is untouched by the menu event and streams to its done.
        frames_until(&mut socket, |frame| frame["type"] == "done").await;
        socket.close(None).await.expect("close the socket");
    }
}
