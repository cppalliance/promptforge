//! The `/ws` WebSocket endpoint: browser chat over bidirectional JSON text
//! frames, relayed through the gateway's streaming chat completion.
//!
//! A client upgrades `GET /ws` and sends chat requests as text frames:
//! `{"type":"chat","model":"...","messages":[...]}`. Each chat frame runs one
//! streaming gateway completion; the session answers with
//! `{"type":"delta","content":"..."}` frames as content arrives, a terminal
//! `{"type":"done"}` when the stream completes, or
//! `{"type":"error","message":"..."}` on any failure - transport, mid-stream,
//! or a gateway that declines the stream with a non-success status. A frame
//! that is not a well-formed chat request is answered with an `error` frame
//! and the session continues. Chat frames are answered strictly in order:
//! while one streams, later frames wait.
//!
//! Exactly one tape event is written per chat frame, after the stream
//! settles and before the terminal frame is sent, so a client holding `done`
//! or `error` can trust the tape to hold the exchange. A client that
//! disconnects mid-stream is taped with a `client disconnected` note beside
//! the partial content.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};

use crate::app::{AppState, StreamTape, delta_content, tape_round_trip, value_from_bytes};
use crate::gateway::{ChatRequest, ChatStream};

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
    // The receive loop and any future server-pushed frames both speak to the
    // client, so outbound messages funnel through one channel into the
    // writer task, mirroring the voice session.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(32);
    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            if sink.send(message).await.is_err() {
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
    tracing::info!(session, "chat session closed");
}

/// Handles one inbound text frame: a well-formed `chat` frame runs a
/// streamed completion, anything else is answered with an `error` frame.
async fn handle_frame(state: &AppState, text: &str, out: &tokio::sync::mpsc::Sender<Message>) {
    let frame: serde_json::Value = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(error) => {
            send_error(out, format!("invalid JSON frame: {error}")).await;
            return;
        }
    };
    if frame.get("type").and_then(serde_json::Value::as_str) != Some("chat") {
        send_error(out, "unknown frame type; expected \"chat\"").await;
        return;
    }
    let request: ChatRequest = match serde_json::from_value(frame.clone()) {
        Ok(request) => request,
        Err(error) => {
            send_error(out, format!("invalid chat request: {error}")).await;
            return;
        }
    };
    relay_chat(state, request, frame, out).await;
}

/// Runs one streaming chat completion against the gateway, forwarding
/// content deltas as `delta` frames and settling with `done` or `error`.
async fn relay_chat(
    state: &AppState,
    request: ChatRequest,
    frame: serde_json::Value,
    out: &tokio::sync::mpsc::Sender<Message>,
) {
    let started = Instant::now();
    let chat_stream = match state
        .gateway_client()
        .chat_completion_stream(&request)
        .await
    {
        Ok(chat_stream) => chat_stream,
        Err(error) => {
            send_error(out, error.to_string()).await;
            return;
        }
    };
    let mut payloads = match chat_stream {
        ChatStream::Stream { payloads, .. } => payloads,
        ChatStream::Relay(upstream) => {
            // The gateway declined the stream with an ordinary response; it
            // is taped like a buffered chat and reported as an error frame.
            let response = value_from_bytes(&upstream.body);
            tape_round_trip(
                state.tape(),
                request.model,
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
            send_error(out, message).await;
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
                let delta = serde_json::json!({"type": "delta", "content": text});
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
                send_error(out, message).await;
                return;
            }
            None => {
                finish.record().await;
                let _ = send_frame(out, serde_json::json!({"type": "done"})).await;
                return;
            }
        }
    }
}

/// Sends one JSON text frame; a false return means the client is gone.
async fn send_frame(out: &tokio::sync::mpsc::Sender<Message>, frame: serde_json::Value) -> bool {
    out.send(Message::Text(frame.to_string().into()))
        .await
        .is_ok()
}

/// Sends one `error` frame carrying `message`, ignoring a dead client.
async fn send_error(out: &tokio::sync::mpsc::Sender<Message>, message: impl Into<String>) {
    let frame = serde_json::json!({"type": "error", "message": message.into()});
    let _ = send_frame(out, frame).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use futures_util::stream;
    use tokio_tungstenite::tungstenite;

    use crate::app::router;
    use crate::config::{Config, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig};

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
    /// free loopback port and returns the `/ws` URL plus the tempdir keeping
    /// the tape alive.
    async fn spawn_chat_server(base_url: &str) -> (String, tempfile::TempDir) {
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
        tokio::spawn(async move {
            axum::serve(listener, router(state))
                .await
                .expect("chat test server serves");
        });
        (format!("ws://{addr}/ws"), tape_dir)
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
        let (url, _tape_dir) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        // The role-priming event carries no content and yields no frame.
        let first = read_frame(&mut socket).await;
        assert_eq!(first, serde_json::json!({"type": "delta", "content": "po"}));
        let second = read_frame(&mut socket).await;
        assert_eq!(
            second,
            serde_json::json!({"type": "delta", "content": "ng"})
        );
        let third = read_frame(&mut socket).await;
        assert_eq!(third, serde_json::json!({"type": "done"}));
        socket.close(None).await.expect("close the socket");
    }

    #[tokio::test]
    async fn a_completed_chat_tapes_one_event_with_the_assembled_response() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (url, tape_dir) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;
        // The terminal frame is sent after the tape write, so holding `done`
        // means the tape is durable.
        loop {
            let frame = read_frame(&mut socket).await;
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
        let (url, tape_dir) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        let first = read_frame(&mut socket).await;
        assert_eq!(first, serde_json::json!({"type": "delta", "content": "po"}));
        let second = read_frame(&mut socket).await;
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
    async fn a_declined_stream_sends_an_error_frame_and_tapes_the_envelope() {
        let base_url = spawn_gateway(
            Router::new().route("/v1/chat/completions", post(mock_chat_declines_stream)),
        )
        .await;
        let (url, tape_dir) = spawn_chat_server(&base_url).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect to /ws");
        send_chat(&mut socket).await;

        let frame = read_frame(&mut socket).await;
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
        let (url, _tape_dir) = spawn_chat_server("http://127.0.0.1:1").await;
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
            let frame = read_frame(&mut socket).await;
            assert_eq!(
                frame["type"], "error",
                "a malformed frame is answered, not fatal: {bad}"
            );
        }
        // The session survives: a well-formed frame still gets through to
        // the (unreachable) gateway and answers with its own error.
        send_chat(&mut socket).await;
        let frame = read_frame(&mut socket).await;
        assert_eq!(frame["type"], "error");
        socket.close(None).await.expect("close the socket");
    }
}
