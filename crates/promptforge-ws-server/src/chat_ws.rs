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
//! status. The `id` is optional and echoed verbatim on every frame of that
//! chat's reply, so one socket can multiplex requests; a frame without an
//! `id` is answered untagged. A frame that is not a well-formed chat
//! request is answered with an `error` frame and the session continues.
//! Chat frames are answered strictly in order: while one streams, later
//! frames wait. A chat received while the heartbeat knows the gateway is
//! down is answered immediately with a "Gateway unreachable" error frame -
//! no upstream attempt, no tape event.
//!
//! One task owns the socket: a single `select!` loop reads inbound frames
//! and writes every outbound frame itself - no outbox channel, no writer
//! task. The in-flight chat's gateway payloads arrive as a branch of the
//! same loop, so status updates from [`crate::status`] and catalog pushes
//! from [`crate::catalog`] keep flowing between deltas while a chat
//! streams. On connect the session first sends the retained status and
//! catalog snapshots, honoring the delivery contract's resend promise
//! (see [`crate::protocol`]); after that both buses forward as they
//! publish, and a session too slow to drain them skips ahead to the
//! newest snapshot rather than slowing the producers.
//!
//! Exactly one tape event is written per chat frame, after the stream
//! settles and before the terminal frame is sent, so a client holding
//! `done` or `error` can trust the tape to hold the exchange. A client
//! that disconnects mid-stream is taped with a `client disconnected` note
//! beside the partial content by the abandoned chat's drop guard.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::StreamExt;
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::gateway::{ChatStream, GatewayError, GatewayResponse, SsePayloadStream};
use crate::protocol::{Activity, ChatRequest, DeltaFrame, DoneFrame, ErrorFrame, ReasoningFrame};
use crate::push::Push;
use crate::relay::{tape_round_trip, value_from_bytes};
use crate::tape::Tape;

/// Chat session ids for log correlation, handed out in connection order.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// Upgrades a `GET /ws` request to a WebSocket chat session.
pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
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
    // The delivery contract resends the current status and catalog on
    // reconnect; the buses retain the newest copy for exactly this send.
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

    let mut chat: Option<ActiveChat> = None;
    // The buses close only when the server state tears down; a closed bus
    // disables its branch rather than spinning the loop on `Closed`.
    let mut status_open = true;
    let mut catalog_open = true;

    loop {
        tokio::select! {
            // Biased, ephemeral branches first: an in-flight chat's payload
            // stream against a local gateway is ready on every poll, and an
            // unbiased select could starve the queued status frames past
            // the chat's `done`. Draining the buses first bounds their
            // staleness at one frame; they cannot starve the durable path
            // in turn because their producers push at human pace.
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
            // The durable path. Chat reply frames are direct per-request
            // replies: the gateway stream feeding them is owned by this
            // loop and fans out to nobody else, so sending them from this
            // branch preserves stream order and delivers exactly - the
            // contract's direct-reply case, which needs no Notify or
            // cursor because no shared transcript state exists to index.
            payload = next_payload(&mut chat) => {
                if !advance_chat(&mut chat, payload, &mut socket, &push).await {
                    break;
                }
            }
            // Inbound frames wait while a chat streams, which keeps chat
            // replies strictly ordered; a client that vanishes mid-stream
            // surfaces as a failed send on the durable branch above.
            inbound = socket.recv(), if chat.is_none() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    chat = handle_frame(&state, &text, &mut socket).await;
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

/// One chat in flight: the gateway's SSE payload stream beside the state
/// its settle paths need. Everything here releases on drop - dropping the
/// payload stream cancels the upstream completion, and the tape guard
/// records the abandoned exchange.
struct ActiveChat {
    payloads: SsePayloadStream,
    /// The request's `id`, echoed on every frame of this chat's reply.
    id: Option<serde_json::Value>,
    tape: StreamTape,
}

/// The next SSE payload of the in-flight chat; pending forever when no
/// chat is in flight, so the select! branch simply never fires.
async fn next_payload(chat: &mut Option<ActiveChat>) -> Option<Result<String, GatewayError>> {
    match chat.as_mut() {
        Some(active) => active.payloads.next().await,
        None => std::future::pending().await,
    }
}

/// Advances the in-flight chat by one payload outcome: forwards deltas,
/// settles the chat when its stream ends or fails. A `false` return means
/// the client is gone and the session loop should end; the abandoned
/// chat's guard then tapes the disconnect.
async fn advance_chat(
    chat: &mut Option<ActiveChat>,
    payload: Option<Result<String, GatewayError>>,
    socket: &mut WebSocket,
    push: &Push,
) -> bool {
    // `next_payload` resolves only while a chat is in flight, so each arm
    // below always finds one.
    match payload {
        Some(Ok(payload)) => {
            // The terminal sentinel ends the wire stream but carries no
            // content; role-priming and usage events have none either.
            if payload == "[DONE]" {
                return true;
            }
            let Some(active) = chat.as_mut() else {
                return true;
            };
            match forward_payload(&payload, active, socket, push).await {
                Forward::Sent => true,
                Forward::ClientGone => false,
            }
        }
        Some(Err(error)) => {
            let Some(active) = chat.take() else {
                return true;
            };
            let message = error.to_string();
            active.tape.record(Some(message.clone())).await;
            push.push_failure("Connection lost", message.clone(), Activity::General);
            send_error(socket, active.id.as_ref(), message).await;
            true
        }
        None => {
            let Some(active) = chat.take() else {
                return true;
            };
            active.tape.record(None).await;
            push.push_idle();
            let _ = send_frame(socket, &DoneFrame::new(active.id.as_ref())).await;
            true
        }
    }
}

/// Handles one inbound text frame: a well-formed `chat` frame opens a
/// streamed completion and returns it as the loop's in-flight chat;
/// anything else is answered with an `error` frame.
async fn handle_frame(state: &AppState, text: &str, socket: &mut WebSocket) -> Option<ActiveChat> {
    let frame: serde_json::Value = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(error) => {
            send_error(socket, None, format!("invalid JSON frame: {error}")).await;
            return None;
        }
    };
    // The request id, echoed on every frame of this chat's reply so one
    // persistent socket can multiplex requests. Absent and null both mean
    // untagged.
    let id = frame.get("id").cloned().filter(|id| !id.is_null());
    if frame.get("type").and_then(serde_json::Value::as_str) != Some("chat") {
        send_error(socket, id.as_ref(), "unknown frame type; expected \"chat\"").await;
        return None;
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
            return None;
        }
    };
    // A gateway the heartbeat knows is down is not attempted: the chat
    // fails fast with a user-visible error instead of a transport error,
    // and nothing is taped because no exchange happened.
    if !state.health().is_reachable() {
        send_error(socket, id.as_ref(), "Gateway unreachable").await;
        return None;
    }
    open_chat(state, request, frame, id, socket).await
}

/// Opens one streaming chat completion against the gateway: returns the
/// in-flight chat when the gateway streams, or settles immediately - an
/// `error` frame, plus a tape event where an exchange happened - and
/// returns nothing.
async fn open_chat(
    state: &AppState,
    request: ChatRequest,
    frame: serde_json::Value,
    id: Option<serde_json::Value>,
    socket: &mut WebSocket,
) -> Option<ActiveChat> {
    let started = Instant::now();
    let push = state.push();
    push.push_status_update(
        "Submitting request...",
        format!("a streaming chat completion from {}", request.model),
        Activity::Thinking,
    );
    let chat_stream = match state
        .gateway_client()
        .chat_completion_stream(&request)
        .await
    {
        Ok(chat_stream) => chat_stream,
        Err(error) => {
            push.push_failure("Connection lost", error.to_string(), Activity::General);
            send_error(socket, id.as_ref(), error.to_string()).await;
            return None;
        }
    };
    match chat_stream {
        ChatStream::Stream { payloads, .. } => {
            push.push_status_update(
                "Streaming response...",
                "the gateway is streaming the reply",
                Activity::Thinking,
            );
            Some(ActiveChat {
                payloads,
                id,
                tape: StreamTape::open(
                    Arc::clone(state.tape()),
                    request.model,
                    frame,
                    started,
                    push,
                ),
            })
        }
        ChatStream::Relay(upstream) => {
            declined_stream(
                state,
                request.model,
                frame,
                upstream,
                started,
                id.as_ref(),
                socket,
            )
            .await;
            None
        }
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
) -> Forward {
    let ActiveChat { id, tape, .. } = active;
    let fields = delta_fields(payload);
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
    state: &AppState,
    model: String,
    frame: serde_json::Value,
    upstream: GatewayResponse,
    started: Instant,
    id: Option<&serde_json::Value>,
    socket: &mut WebSocket,
) {
    let response = value_from_bytes(&upstream.body);
    tape_round_trip(
        state.tape(),
        model,
        frame,
        response.clone(),
        started.elapsed(),
    )
    .await;
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
    state.push().push_failure(
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
/// contribute nothing to the assembled response. Reasoning models stream
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
}
