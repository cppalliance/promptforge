//! The `/ws` WebSocket endpoint: one persistent socket carrying all
//! downstream JSON - browser chat over bidirectional text frames, relayed
//! through the gateway's streaming chat completion, plus unsolicited status
//! updates from the observer.
//!
//! A client upgrades `GET /ws` once and sends chat requests as text frames:
//! `{"type":"chat","id":N,"model":"...","messages":[...]}`. Each chat frame
//! runs one streaming gateway completion; the session answers with
//! `{"type":"delta","content":"...","id":N}` frames as content arrives, a
//! terminal `{"type":"done","id":N}` when the stream completes, or
//! `{"type":"error","message":"...","id":N}` on any failure - transport,
//! mid-stream, or a gateway that declines the stream with a non-success
//! status. The `id` is optional and echoed verbatim on every frame of that
//! chat's reply, so one socket can multiplex requests; a frame without an
//! `id` is answered untagged. A frame that is not a well-formed chat
//! request is answered with an `error` frame and the session continues.
//! Chat frames are answered strictly in order: while one streams, later
//! frames wait.
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;

use crate::app::{AppState, tape_round_trip, value_from_bytes};
use crate::gateway::{ChatRequest, ChatStream, GatewayResponse};
use crate::status::Activity;
use crate::tape::Tape;

/// Session ids for log correlation, handed out in connection order.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

/// Upgrades a `GET /ws` request to a WebSocket chat session.
pub(crate) async fn upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    ws.on_upgrade(move |socket| run_session(session, socket, state))
}

/// Runs one chat session until the socket closes or fails.
async fn run_session(session: u64, socket: WebSocket, state: AppState) {
    tracing::info!(session, "chat session opened");
    let (mut sink, mut stream) = socket.split();
    // The receive loop and the status forwarder both speak to the client,
    // so outbound messages funnel through one channel into the writer task,
    // mirroring the voice session.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(32);
    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    // Status frames and catalog pushes are unsolicited and must flow while
    // a chat relay has the inbound loop parked, so they get their own task
    // off the broadcast buses rather than a branch in that loop. A client
    // too slow to keep up lags the rings and skips ahead; the buses never
    // block for it.
    let mut status_rx = state.status().subscribe();
    let mut catalog_rx = state.catalog().subscribe();
    let status_out = out_tx.clone();
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
                        tracing::debug!(session, skipped, "status receiver lagged; skipped updates");
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
                        tracing::debug!(session, skipped, "catalog receiver lagged; skipped pushes");
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
            Ok(Message::Text(text)) => handle_frame(&state, &text, &out_tx).await,
            // Binary frames carry no chat meaning; pings and pongs are
            // answered by axum itself.
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_)) => {}
            Ok(Message::Close(_)) => break,
            Err(error) => {
                tracing::warn!(session, %error, "chat session socket failed");
                break;
            }
        }
    }
    drop(out_tx);
    writer.abort();
    forwarder.abort();
    tracing::info!(session, "chat session closed");
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
    let status = state.status();
    status.info(
        "Submitting request...",
        format!("a streaming chat completion from {}", request.model),
        Activity::Gateway,
    );
    let chat_stream = match state
        .gateway_client()
        .chat_completion_stream(&request)
        .await
    {
        Ok(chat_stream) => chat_stream,
        Err(error) => {
            status.error("Connection lost", error.to_string(), Activity::Gateway);
            send_error(out, id.as_ref(), error.to_string()).await;
            return;
        }
    };
    let mut payloads = match chat_stream {
        ChatStream::Stream { payloads, .. } => {
            status.info(
                "Streaming response...",
                "the gateway is streaming the reply",
                Activity::Gateway,
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
                let Some(text) = delta_content(&payload) else {
                    continue;
                };
                finish.assembled.push_str(&text);
                // A chunk pulse at Debug: the UI ignores the text, but the
                // activity field keeps the gateway indicator alive.
                status.debug(
                    "Streaming response...",
                    "a gateway response chunk",
                    Activity::Gateway,
                );
                let delta = tagged(
                    id.as_ref(),
                    serde_json::json!({"type": "delta", "content": text}),
                );
                if !send_frame(out, delta).await {
                    finish.error = Some("client disconnected mid-stream".to_string());
                    finish.record().await;
                    return;
                }
            }
            Some(Err(error)) => {
                let message = error.to_string();
                finish.error = Some(message.clone());
                finish.record().await;
                status.error("Connection lost", message.clone(), Activity::Gateway);
                send_error(out, id.as_ref(), message).await;
                return;
            }
            None => {
                finish.record().await;
                status.idle();
                let _ = send_frame(
                    out,
                    tagged(id.as_ref(), serde_json::json!({"type": "done"})),
                )
                .await;
                return;
            }
        }
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
    state.status().error(
        format!("Gateway error: {}", upstream.status),
        message.clone(),
        Activity::Gateway,
    );
    send_error(out, id, message).await;
}

/// Tags a reply frame with the request's `id`, when it carried one.
fn tagged(id: Option<&serde_json::Value>, mut frame: serde_json::Value) -> serde_json::Value {
    if let (Some(id), Some(object)) = (id, frame.as_object_mut()) {
        object.insert("id".to_string(), id.clone());
    }
    frame
}

/// Extracts the text delta from one gateway SSE payload, if it carries
/// content.
///
/// Role-priming and usage events have no `choices[0].delta.content` and
/// contribute nothing to the assembled response.
fn delta_content(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let content = value
        .get("choices")?
        .as_array()?
        .first()?
        .get("delta")?
        .get("content")?
        .as_str()?;
    Some(content.to_string())
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
async fn send_frame(out: &tokio::sync::mpsc::Sender<Message>, frame: serde_json::Value) -> bool {
    out.send(Message::Text(frame.to_string().into()))
        .await
        .is_ok()
}

/// Sends one `error` frame carrying `message`, tagged with the request's
/// `id` when there is one, ignoring a dead client.
async fn send_error(
    out: &tokio::sync::mpsc::Sender<Message>,
    id: Option<&serde_json::Value>,
    message: impl Into<String>,
) {
    let frame = tagged(
        id,
        serde_json::json!({"type": "error", "message": message.into()}),
    );
    let _ = send_frame(out, frame).await;
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
    use futures_util::stream;
    use tokio_tungstenite::tungstenite;

    use crate::app::router;
    use crate::config::{Config, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig};
    use crate::status::{Activity, Progress, Severity, StatusBarUpdate};

    const STREAM_BODY: &str = concat!(
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"po\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ng\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    const UPSTREAM_ERROR: &str =
        r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;

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

    /// Binds the workbench router against the gateway at `base_url` on a
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
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;
        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(first["type"], "delta");
        // Drop the socket without a close handshake; the server notices when
        // a later delta send fails.
        drop(socket);

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
            activity: Activity::Gateway,
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
                "activity": "gateway",
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
            state.status(),
            state.health().clone(),
            state.catalog(),
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
            let frame = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut socket))
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
