//! The temporary direct-gateway chat adapter: the session's chat
//! multiplexing and execution, relayed straight through the gateway
//! client.
//!
//! One in-flight chat is an [`ActiveChat`]: the gateway work in
//! whichever phase it is in, beside the state the settle paths need.
//! The [`Chats`] map holds every in-flight chat, and the merged
//! [`next_event`] poll drives them round-robin, one event per turn, so
//! neither a hot stream nor a parked admission can starve its neighbors.
//! Chat execution stays behind this boundary: when chat moves from the
//! gateway to PromptForge, the session's socket ownership, buses, and
//! menu events are untouched.

mod delta;
mod tape;

use std::sync::Arc;
use std::task::Poll;
use std::time::Instant;

use axum::extract::ws::WebSocket;
use futures_util::future::BoxFuture;
use futures_util::{FutureExt, StreamExt};

use crate::app::AppState;
use crate::backoff::ReconnectBackoff;
use crate::gateway::{ChatStream, GatewayError, GatewayResponse, SsePayloadStream};
use crate::protocol::{Activity, ChatRequest, DeltaFrame, DoneFrame, ReasoningFrame};
use crate::push::Push;
use crate::relay::value_from_bytes;

use self::delta::delta_fields;
use self::tape::StreamTape;
use super::{send_error, send_frame};

/// One chat in flight - one entry of the session's chat map: the
/// gateway work in whichever phase it is in, beside the state the settle
/// paths need. Everything here releases on drop - dropping the work
/// cancels the upstream completion (a pending open aborts its HTTP
/// request, a stream closes its body), and the tape guard records the
/// abandoned exchange.
pub(super) struct ActiveChat {
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
pub(super) enum ChatKey {
    /// The single chat allowed to stream without an `id`.
    Untagged,
    /// A chat named by its request `id`, keyed by the id's JSON text.
    Tagged(String),
}

impl ChatKey {
    /// Derives the key from a request's optional `id`.
    pub(super) fn from_id(id: Option<&serde_json::Value>) -> Self {
        match id {
            Some(id) => Self::Tagged(id.to_string()),
            None => Self::Untagged,
        }
    }

    /// The refusal sent for a chat frame that collides with this live
    /// key, naming the rule it broke.
    pub(super) fn refusal(&self) -> String {
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
pub(super) struct Chats {
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
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
        }
    }

    /// Whether no chat is in flight.
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a chat with this key is streaming right now.
    pub(super) fn is_live(&self, key: &ChatKey) -> bool {
        self.entries.iter().any(|(live, _)| live == key)
    }

    /// Adds a newly opened chat to the map.
    pub(super) fn insert(&mut self, key: ChatKey, chat: ActiveChat) {
        self.entries.push((key, chat));
    }

    /// Removes and returns the chat with this key, if one is in flight.
    pub(super) fn remove(&mut self, key: &ChatKey) -> Option<ActiveChat> {
        let index = self.entries.iter().position(|(live, _)| live == key)?;
        Some(self.entries.remove(index).1)
    }
}

/// One step of one in-flight chat, yielded by the merged poll.
pub(super) enum ChatEvent {
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
pub(super) async fn next_event(chats: &mut Chats) -> (usize, ChatEvent) {
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
pub(super) async fn advance_chat(
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

/// Tears down the one chat a `cancel` frame names: dropping its gateway
/// work cancels the upstream completion - a streaming chat closes its
/// payload stream, a chat still waiting for admission aborts its queued
/// request - and its tape records the abandonment beside the partial
/// content (empty for a chat that never streamed) - the same teardown a
/// disconnect performs, scoped to one chat. A cancel for an unknown or
/// already-settled chat is ignored with a debug log, because a cancel
/// racing its own `done` is normal.
pub(super) async fn cancel_chat(
    session: u64,
    id: Option<&serde_json::Value>,
    chats: &mut Chats,
    push: &Push,
) {
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

/// Posts one streaming chat completion to the gateway and returns the
/// chat in `Opening` state, its tape guard already armed. Nothing is
/// awaited here: the returned chat's open future resolves in the session
/// loop's merged branch, where [`advance_chat`] either transitions it to
/// `Streaming` or settles it - an `error` frame, plus a tape event where
/// an exchange happened. Arming the guard before the gateway answers is
/// what gives a chat canceled or abandoned while still queued its one
/// tape event.
pub(super) fn begin_chat(
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
