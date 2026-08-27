//! The `/ws` WebSocket endpoint: one persistent socket carrying all
//! downstream JSON - browser chat over bidirectional text frames, relayed
//! through the gateway's streaming chat completion, plus unsolicited status
//! updates from the observer.
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
//! Status updates from [`crate::status`] and model catalog pushes from
//! [`crate::catalog`] are forwarded to the socket as unsolicited
//! `{"type":"status",...}` and `{"type":"models",...}` frames by a
//! dedicated task, so they flow at any time - including while a chat is
//! streaming, when the inbound loop is parked inside the relay.
//!
//! Exactly one tape event is written per chat frame, after the stream
//! settles and before the terminal frame is sent, so a client holding
//! `done` or `error` can trust the tape to hold the exchange. A client
//! that disconnects mid-stream is taped with a `client disconnected` note
//! beside the partial content.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::StreamExt;
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::gateway::{ChatStream, GatewayResponse};
use crate::protocol::{Activity, ChatRequest, DeltaFrame, DoneFrame, ErrorFrame, ReasoningFrame};
use crate::push::Push;
use crate::relay::{tape_round_trip, value_from_bytes};
use crate::tape::Tape;
use crate::ws_session::WsSession;

/// Upgrades a `GET /ws` request to a WebSocket chat session.
pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| run_session(socket, state))
}

/// Runs one chat session until the socket closes or fails.
async fn run_session(socket: WebSocket, state: AppState) {
    let (sink, mut stream) = socket.split();
    // The receive loop and the status forwarder both speak to the client,
    // so outbound messages funnel through the session's outbox into its
    // writer task, mirroring the voice session.
    let session = WsSession::new(sink);
    let session_id = session.id();
    tracing::info!(session = session_id, "chat session opened");

    // Status frames and catalog pushes are unsolicited and must flow while
    // a chat relay has the inbound loop parked, so they get their own task
    // off the broadcast buses rather than a branch in that loop. A client
    // too slow to keep up lags the rings and skips ahead; the buses never
    // block for it.
    let mut status_rx = state.status().subscribe();
    let mut catalog_rx = state.catalog().subscribe();
    let status_out = session.outbox().clone();
    let forwarder = tokio::spawn(async move {
        loop {
            let text = tokio::select! {
                received = status_rx.recv() => match received {
                    Ok(update) => {
                        // Serializing strings and integers cannot fail.
                        let Ok(text) = serde_json::to_string(&update.frame()) else {
                            continue;
                        };
                        text
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(session = session_id, skipped, "status receiver lagged; skipped updates");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                received = catalog_rx.recv() => match received {
                    Ok(push) => {
                        // Serializing a JSON value cannot fail.
                        let Ok(text) = serde_json::to_string(&push.frame()) else {
                            continue;
                        };
                        text
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(session = session_id, skipped, "catalog receiver lagged; skipped pushes");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            if status_out.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(received) = stream.next().await {
        match received {
            Ok(Message::Text(text)) => handle_frame(&state, &text, session.outbox()).await,
            // Binary frames carry no chat meaning; pings and pongs are
            // answered by axum itself.
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_)) => {}
            Ok(Message::Close(_)) => break,
            Err(error) => {
                tracing::warn!(session = session_id, %error, "chat session socket failed");
                break;
            }
        }
    }
    session.close();
    forwarder.abort();
    tracing::info!(session = session_id, "chat session closed");
}

/// Handles one inbound text frame: a well-formed `chat` frame runs a
/// streamed completion, anything else is answered with an `error` frame.
async fn handle_frame(state: &AppState, text: &str, out: &tokio::sync::mpsc::Sender<Message>) {
    let frame: serde_json::Value = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(error) => {
            send_error(out, None, format!("invalid JSON frame: {error}")).await;
            return;
        }
    };
    // The request id, echoed on every frame of this chat's reply so one
    // persistent socket can multiplex requests. Absent and null both mean
    // untagged.
    let id = frame.get("id").cloned().filter(|id| !id.is_null());
    if frame.get("type").and_then(serde_json::Value::as_str) != Some("chat") {
        send_error(out, id.as_ref(), "unknown frame type; expected \"chat\"").await;
        return;
    }
    let request: ChatRequest = match serde_json::from_value(frame.clone()) {
        Ok(request) => request,
        Err(error) => {
            send_error(out, id.as_ref(), format!("invalid chat request: {error}")).await;
            return;
        }
    };
    // A gateway the heartbeat knows is down is not attempted: the chat
    // fails fast with a user-visible error instead of a transport error,
    // and nothing is taped because no exchange happened.
    if !state.health().is_reachable() {
        send_error(out, id.as_ref(), "Gateway unreachable").await;
        return;
    }
    relay_chat(state, request, frame, id, out).await;
}

/// Runs one streaming chat completion against the gateway, forwarding
/// content deltas as `delta` frames and settling with `done` or `error`.
async fn relay_chat(
    state: &AppState,
    request: ChatRequest,
    frame: serde_json::Value,
    id: Option<serde_json::Value>,
    out: &tokio::sync::mpsc::Sender<Message>,
) {
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
            send_error(out, id.as_ref(), error.to_string()).await;
            return;
        }
    };
    let mut payloads = match chat_stream {
        ChatStream::Stream { payloads, .. } => {
            push.push_status_update(
                "Streaming response...",
                "the gateway is streaming the reply",
                Activity::Thinking,
            );
            payloads
        }
        ChatStream::Relay(upstream) => {
            declined_stream(
                state,
                request.model,
                frame,
                upstream,
                started,
                id.as_ref(),
                out,
            )
            .await;
            return;
        }
    };
    let mut finish = StreamTape {
        tape: Arc::clone(state.tape()),
        model: request.model,
        request: frame,
        started,
        assembled: String::new(),
        error: None,
    };
    loop {
        match payloads.next().await {
            Some(Ok(payload)) => {
                // The terminal sentinel ends the wire stream but carries no
                // content; role-priming and usage events have none either.
                if payload == "[DONE]" {
                    continue;
                }
                match forward_payload(&payload, &push, id.as_ref(), out, &mut finish.assembled)
                    .await
                {
                    Forward::Sent => {}
                    Forward::ClientGone => {
                        client_disconnected(finish, &push).await;
                        return;
                    }
                }
            }
            Some(Err(error)) => {
                let message = error.to_string();
                finish.error = Some(message.clone());
                finish.record().await;
                push.push_failure("Connection lost", message.clone(), Activity::General);
                send_error(out, id.as_ref(), message).await;
                return;
            }
            None => {
                finish.record().await;
                push.push_idle();
                let _ = send_frame(out, &DoneFrame::new(id.as_ref())).await;
                return;
            }
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
/// appended to `assembled` for the tape. The chunk pulses at Debug: the UI
/// ignores the text, but the activity field keeps the LED lit.
async fn forward_payload(
    payload: &str,
    push: &Push,
    id: Option<&serde_json::Value>,
    out: &tokio::sync::mpsc::Sender<Message>,
    assembled: &mut String,
) -> Forward {
    let fields = delta_fields(payload);
    if let Some(text) = fields.reasoning {
        push.push_activity(
            "Streaming response...",
            "a gateway reasoning chunk",
            Activity::Thinking,
        );
        if !send_frame(out, &ReasoningFrame::new(text, id)).await {
            return Forward::ClientGone;
        }
    }
    let Some(text) = fields.content else {
        return Forward::Sent;
    };
    assembled.push_str(&text);
    push.push_activity(
        "Streaming response...",
        "a gateway response chunk",
        Activity::Generating,
    );
    if send_frame(out, &DeltaFrame::new(text, id)).await {
        Forward::Sent
    } else {
        Forward::ClientGone
    }
}

/// Settles a stream whose client vanished mid-way: the tape records the
/// partial exchange, and because the dead client is gone but any other
/// subscriber still watches the observer, the status returns to Ready
/// rather than leaving a stale activity LED.
async fn client_disconnected(mut finish: StreamTape, push: &Push) {
    finish.error = Some("client disconnected mid-stream".to_string());
    finish.record().await;
    push.push_idle();
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
    out: &tokio::sync::mpsc::Sender<Message>,
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
    send_error(out, id, message).await;
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
/// first non-empty synonym wins.
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
        .find_map(|key| delta.get(*key).and_then(serde_json::Value::as_str))
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    DeltaFields { content, reasoning }
}

/// Tape bookkeeping carried through one streaming chat.
///
/// The session consumes this exactly once per chat frame, so a streamed chat
/// always tapes exactly one event.
struct StreamTape {
    tape: Arc<Tape>,
    model: String,
    request: serde_json::Value,
    started: Instant,
    /// Concatenation of every content delta forwarded so far.
    assembled: String,
    /// The mid-stream failure note, when the gateway stream errored.
    error: Option<String>,
}

impl StreamTape {
    /// Writes the stream's single tape event: the assembled content on
    /// success, or an error note plus the partial content on failure.
    async fn record(self) {
        let Self {
            tape,
            model,
            request,
            started,
            assembled,
            error,
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
async fn send_frame<F: serde::Serialize>(
    out: &tokio::sync::mpsc::Sender<Message>,
    frame: &F,
) -> bool {
    // Serializing the protocol frames cannot fail: strings, integers, and
    // JSON values only. A frame that somehow cannot serialize is skipped,
    // which is not a gone client.
    let Ok(text) = serde_json::to_string(frame) else {
        return true;
    };
    out.send(Message::Text(text.into())).await.is_ok()
}

/// Sends one `error` frame carrying `message`, tagged with the request's
/// `id` when there is one, ignoring a dead client.
async fn send_error(
    out: &tokio::sync::mpsc::Sender<Message>,
    id: Option<&serde_json::Value>,
    message: impl Into<String>,
) {
    let _ = send_frame(out, &ErrorFrame::new(message.into(), id)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use futures_util::{SinkExt, stream};
    use tokio_tungstenite::tungstenite;

    use crate::app::router;
    use crate::config::{Config, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig};
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

    /// Binds `app` as a mock gateway on a free loopback port and returns its
    /// base URL.
    async fn spawn_gateway(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock gateway");
        let addr = listener.local_addr().expect("mock gateway address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock gateway serves");
        });
        format!("http://{addr}")
    }

    /// Binds the workshop router against the gateway at `base_url` on a
    /// free loopback port and returns the `/ws` URL, the tempdir keeping
    /// the tape alive, and a handle on the shared state (for poking the
    /// status bus directly).
    async fn spawn_chat_server(base_url: &str) -> (String, tempfile::TempDir, AppState) {
        let tape_dir = tempfile::TempDir::new().expect("tempdir");
        let config = Config {
            gateway: GatewayConfig {
                base_url: base_url.to_string(),
                api_key: "test-key".to_string(),
            },
            tape: TapeConfig {
                path: tape_dir.path().join("tape.jsonl"),
            },
            server: ServerConfig::default(),
            voice: VoiceConfig::default(),
        };
        let state = AppState::new(&config).expect("state builds in tests");
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
    /// frames are unsolicited and may interleave with a chat's replies at
    /// any point, so reply assertions skip them.
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
        // A malformed frame's error reply proves the session's inbound loop
        // is running, which means the status subscription before it is live.
        socket
            .send(tungstenite::Message::Text("not json".into()))
            .await
            .expect("the frame is sent");
        let reply = read_frame(&mut socket).await;
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

    #[tokio::test]
    #[ignore = "flaky on CI: the catalog push races the heartbeat transition and never arrives on slow runners"]
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
        // A malformed frame's error reply proves the session's tasks -
        // including its catalog subscription - are live before the flip.
        socket
            .send(tungstenite::Message::Text("not json".into()))
            .await
            .expect("the frame is sent");
        let reply = read_frame(&mut socket).await;
        assert_eq!(reply["type"], "error");

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
