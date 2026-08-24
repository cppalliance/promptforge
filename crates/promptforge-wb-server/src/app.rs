//! The axum router, handlers, and serving loop for the workbench server.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use futures_util::stream::{self, StreamExt};

use crate::config::Config;
use crate::gateway::{ChatRequest, ChatStream, GatewayClient, GatewayError, GatewayResponse};
use crate::tape::{Tape, TapeError, TapeEvent};

/// Address the server binds to when no override is given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7910";

/// Shared handler state: the authenticated gateway client and the session
/// tape.
#[derive(Debug, Clone)]
pub struct AppState {
    gateway: GatewayClient,
    tape: Arc<Tape>,
}

impl AppState {
    /// Builds shared state from the loaded configuration.
    ///
    /// # Errors
    /// Returns [`AppError::Gateway`] if the HTTP client cannot be built and
    /// [`AppError::Tape`] if the session tape cannot be opened.
    pub fn new(config: &Config) -> Result<Self, AppError> {
        let gateway = GatewayClient::new(&config.gateway.base_url, &config.gateway.api_key)
            .map_err(AppError::Gateway)?;
        let tape = Tape::open(&config.tape.path).map_err(AppError::Tape)?;
        Ok(Self {
            gateway,
            tape: Arc::new(tape),
        })
    }
}

/// A shared-state construction failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    /// The gateway HTTP client could not be built.
    #[non_exhaustive]
    #[error("build gateway client")]
    Gateway(#[source] GatewayError),

    /// The session tape could not be opened.
    #[non_exhaustive]
    #[error("open session tape")]
    Tape(#[source] TapeError),
}

/// Returns the workbench server router with every route mounted.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(ui_index))
        .route("/app.js", get(ui_app_js))
        .route("/style.css", get(ui_style_css))
        .route("/markdown-it.min.js", get(ui_markdown_it))
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/chat", post(chat))
        .with_state(state)
}

/// Binds to `bind` and serves until the process is stopped.
///
/// # Errors
/// Returns `std::io::Error` if the bind fails or the server stops with an
/// error.
pub async fn run(state: AppState, bind: &str) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(state)).await
}

/// Answers the health probe with a static JSON body.
async fn health() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"status":"serving"}"#,
    )
}

/// Serves the chat UI's `index.html`, embedded into the binary.
async fn ui_index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../ui/index.html"),
    )
}

/// Serves the chat UI's application script, embedded into the binary.
async fn ui_app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../ui/app.js"),
    )
}

/// Serves the chat UI's stylesheet, embedded into the binary.
async fn ui_style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../ui/style.css"),
    )
}

/// Serves the vendored markdown-it 14.1.0 renderer, embedded into the
/// binary.
async fn ui_markdown_it() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../ui/markdown-it.min.js"),
    )
}

/// Relays the gateway's model catalog to the caller verbatim.
async fn models(State(state): State<AppState>) -> Response {
    relay(state.gateway.list_models().await)
}

/// Forwards a chat completion to the gateway, tapes the round-trip, and
/// relays the reply verbatim.
///
/// A completed round-trip is recorded on the session tape; a tape failure is
/// logged and never changes the response. A request carrying
/// `"stream": true` is answered with a workbench SSE stream instead.
async fn chat(State(state): State<AppState>, body: String) -> Response {
    let request_value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => return bad_request(&error),
    };
    let request: ChatRequest = match serde_json::from_value(request_value.clone()) {
        Ok(request) => request,
        Err(error) => return bad_request(&error),
    };
    if wants_stream(&request_value) {
        return chat_stream(state, request, request_value).await;
    }
    let started = Instant::now();
    let result = state.gateway.chat_completion(&request).await;
    let latency = started.elapsed();
    if let Ok(upstream) = &result {
        let response_value = value_from_bytes(&upstream.body);
        tape_round_trip(
            &state.tape,
            request.model,
            request_value,
            response_value,
            latency,
        )
        .await;
    }
    relay(result)
}

/// Returns true when the client asked for an SSE stream with
/// `"stream": true` in the request JSON.
fn wants_stream(request: &serde_json::Value) -> bool {
    request.get("stream").and_then(serde_json::Value::as_bool) == Some(true)
}

/// Forwards a streaming chat completion as a workbench SSE stream.
///
/// The gateway's SSE payloads are relayed event-for-event as they arrive,
/// including the terminal `[DONE]`. Exactly one tape event is written when
/// the stream ends: the assembled content on success, or an error note when
/// the gateway stream fails mid-way. A gateway that declines the stream with
/// a non-success status is relayed and taped exactly like a buffered chat.
async fn chat_stream(
    state: AppState,
    request: ChatRequest,
    request_value: serde_json::Value,
) -> Response {
    let started = Instant::now();
    let result = state.gateway.chat_completion_stream(&request).await;
    let chat_stream = match result {
        Ok(chat_stream) => chat_stream,
        Err(error) => return relay(Err(error)),
    };
    match chat_stream {
        ChatStream::Relay(upstream) => {
            let latency = started.elapsed();
            let response_value = value_from_bytes(&upstream.body);
            tape_round_trip(
                &state.tape,
                request.model,
                request_value,
                response_value,
                latency,
            )
            .await;
            relay(Ok(upstream))
        }
        ChatStream::Stream { status, payloads } => {
            let finish = StreamTape {
                tape: Arc::clone(&state.tape),
                model: request.model,
                request: request_value,
                started,
                assembled: String::new(),
                error: None,
            };
            let events = stream::unfold(
                (payloads, finish),
                |(mut payloads, mut finish)| async move {
                    match payloads.next().await {
                        Some(Ok(payload)) => {
                            if payload != "[DONE]"
                                && let Some(text) = delta_content(&payload)
                            {
                                finish.assembled.push_str(&text);
                            }
                            let event =
                                Ok::<_, std::convert::Infallible>(Event::default().data(payload));
                            Some((event, (payloads, finish)))
                        }
                        Some(Err(error)) => {
                            finish.error = Some(error.to_string());
                            finish.record().await;
                            None
                        }
                        None => {
                            finish.record().await;
                            None
                        }
                    }
                },
            );
            (status, Sse::new(events)).into_response()
        }
    }
}

/// Extracts the text delta from one SSE payload, if it carries content.
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

/// Parses a gateway body as JSON, falling back to a plain string.
fn value_from_bytes(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(body).into_owned()))
}

/// Records one chat round-trip on the session tape.
///
/// A tape failure is logged and never changes the response.
async fn tape_round_trip(
    tape: &Arc<Tape>,
    model: String,
    request: serde_json::Value,
    response: serde_json::Value,
    latency: Duration,
) {
    let written = {
        let tape = Arc::clone(tape);
        tokio::task::spawn_blocking(move || {
            let event = TapeEvent::chat(model, request, response, latency)?;
            tape.record(&event)
        })
        .await
    };
    match written {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(%error, "session tape event was not recorded"),
        Err(error) => tracing::error!(%error, "session tape writer did not finish"),
    }
}

/// Tape bookkeeping carried through one streaming chat's SSE body stream.
///
/// The stream's finalizer consumes this exactly once, so a streamed chat
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

/// Renders the 400 envelope for an unparseable chat body.
fn bad_request(error: &serde_json::Error) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "error": {
                "message": format!("invalid chat request: {error}"),
                "code": "bad_request",
            }
        })
        .to_string(),
    )
        .into_response()
}

/// Turns a gateway call outcome into the workbench's HTTP response.
///
/// Success (any status) is relayed byte-for-byte; a transport failure
/// becomes `502 Bad Gateway` with a small JSON error envelope.
fn relay(result: Result<GatewayResponse, GatewayError>) -> Response {
    match result {
        Ok(upstream) => (
            upstream.status,
            [(header::CONTENT_TYPE, "application/json")],
            upstream.body,
        )
            .into_response(),
        Err(error) => (
            axum::http::StatusCode::BAD_GATEWAY,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": {
                    "message": error.to_string(),
                    "code": "gateway_unreachable",
                }
            })
            .to_string(),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    use axum::Json;
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderMap, Request, StatusCode};
    use tower::ServiceExt;

    use crate::config::{GatewayConfig, ServerConfig, TapeConfig};

    const CATALOG: &str = r#"{"object":"list","data":[{"id":"test-model","object":"model","created":1,"owned_by":"promptforge"}]}"#;
    const COMPLETION: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
    const UPSTREAM_ERROR: &str =
        r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;
    const CHAT_BODY: &str =
        r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}]}"#;
    const STREAM_CHAT_BODY: &str =
        r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}],"stream":true}"#;
    const STREAM_BODY: &str = concat!(
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"po\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ng\"}}]}\n\n",
        "data: [DONE]\n\n",
    );

    fn config_for(base_url: &str, tape_path: &Path) -> Config {
        Config {
            gateway: GatewayConfig {
                base_url: base_url.to_string(),
                api_key: "test-key".to_string(),
            },
            tape: TapeConfig {
                path: tape_path.to_path_buf(),
            },
            server: ServerConfig::default(),
        }
    }

    /// Builds state whose tape lives in a fresh tempdir, returned alongside
    /// so the directory outlives the test.
    fn state_for(base_url: &str) -> (AppState, tempfile::TempDir) {
        let tape_dir = tempfile::TempDir::new().expect("tempdir");
        let config = config_for(base_url, &tape_dir.path().join("tape.jsonl"));
        let state = AppState::new(&config).expect("state builds in tests");
        (state, tape_dir)
    }

    fn authorized(headers: &HeaderMap) -> bool {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some("Bearer test-key")
    }

    async fn mock_models(headers: HeaderMap) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        ([(header::CONTENT_TYPE, "application/json")], CATALOG).into_response()
    }

    async fn mock_chat(headers: HeaderMap, Json(body): Json<serde_json::Value>) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        assert_eq!(body["model"], "test-model");
        assert!(body["messages"].is_array());
        ([(header::CONTENT_TYPE, "application/json")], COMPLETION).into_response()
    }

    async fn mock_broken_models() -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            UPSTREAM_ERROR,
        )
            .into_response()
    }

    async fn mock_chat_not_json(headers: HeaderMap) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        (
            [(header::CONTENT_TYPE, "text/plain")],
            "gateway replied in plain text",
        )
            .into_response()
    }

    async fn mock_chat_stream(headers: HeaderMap, Json(body): Json<serde_json::Value>) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        assert_eq!(body["stream"], true, "the stream flag is forwarded");
        ([(header::CONTENT_TYPE, "text/event-stream")], STREAM_BODY).into_response()
    }

    /// Answers with one good SSE event, then aborts the body mid-stream.
    ///
    /// The pause after the first chunk gives hyper time to flush the headers
    /// and the event before the body errors, so the client observes a stream
    /// that fails mid-way rather than a connection that never answered.
    async fn mock_chat_stream_dies(
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
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
                    tokio::time::sleep(Duration::from_millis(100)).await;
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
    async fn mock_chat_declines_stream(
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
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

    async fn spawn_mock_gateway() -> String {
        spawn_gateway(
            Router::new()
                .route("/v1/models", get(mock_models))
                .route("/v1/chat/completions", post(mock_chat)),
        )
        .await
    }

    async fn spawn_broken_mock_gateway() -> String {
        spawn_gateway(Router::new().route("/v1/models", get(mock_broken_models))).await
    }

    async fn body_bytes(response: Response) -> axum::body::Bytes {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is in memory already")
    }

    fn chat_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(CHAT_BODY))
            .expect("static request parts are valid")
    }

    fn stream_chat_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(STREAM_CHAT_BODY))
            .expect("static request parts are valid")
    }

    #[test]
    fn default_bind_is_loopback_port_7910() {
        assert_eq!(DEFAULT_ADDR, "127.0.0.1:7910");
    }

    /// Asserts a static UI route answers 200 with the expected content type
    /// and a non-empty body.
    async fn assert_ui_asset(uri: &str, expected_content_type: &str) {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK, "{uri} serves");
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap_or_else(|| panic!("{uri} sets content-type"));
        assert_eq!(content_type, expected_content_type, "{uri} content type");
        assert!(
            !body_bytes(response).await.is_empty(),
            "{uri} body is non-empty"
        );
    }

    #[tokio::test]
    async fn index_is_served_at_the_root() {
        assert_ui_asset("/", "text/html; charset=utf-8").await;
    }

    #[tokio::test]
    async fn app_js_is_served_as_javascript() {
        assert_ui_asset("/app.js", "text/javascript; charset=utf-8").await;
    }

    #[tokio::test]
    async fn style_css_is_served_as_css() {
        assert_ui_asset("/style.css", "text/css; charset=utf-8").await;
    }

    #[tokio::test]
    async fn vendored_markdown_it_is_served_as_javascript() {
        assert_ui_asset("/markdown-it.min.js", "text/javascript; charset=utf-8").await;
    }

    #[tokio::test]
    async fn health_returns_serving() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("the health handler sets content-type");
        assert_eq!(content_type, "application/json");
        assert_eq!(&body_bytes(response).await[..], br#"{"status":"serving"}"#);
    }

    #[tokio::test]
    async fn models_are_relayed_byte_for_byte() {
        let base_url = spawn_mock_gateway().await;
        let (state, _tape_dir) = state_for(&base_url);
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], CATALOG.as_bytes());
    }

    #[tokio::test]
    async fn chat_completions_are_relayed_byte_for_byte() {
        let base_url = spawn_mock_gateway().await;
        let (state, _tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], COMPLETION.as_bytes());
    }

    #[tokio::test]
    async fn gateway_error_status_is_relayed_byte_for_byte() {
        let base_url = spawn_broken_mock_gateway().await;
        let (state, _tape_dir) = state_for(&base_url);
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(&body_bytes(response).await[..], UPSTREAM_ERROR.as_bytes());
    }

    #[tokio::test]
    async fn unreachable_gateway_becomes_bad_gateway() {
        // Port 1 is never listening, so the connect fails deterministically.
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "gateway_unreachable");
    }

    #[tokio::test]
    async fn malformed_chat_body_is_a_bad_request() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"not_model":true}"#))
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chat_round_trip_writes_exactly_one_tape_event() {
        let base_url = spawn_mock_gateway().await;
        let (state, tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);

        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        assert!(raw.ends_with('\n'), "the tape line is complete: {raw:?}");
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 1, "exactly one event per round-trip");
        let event: serde_json::Value =
            serde_json::from_str(lines[0]).expect("the tape line is valid JSON");
        assert_eq!(event["kind"], "chat");
        assert_eq!(event["model"], "test-model");
        assert_eq!(event["request"]["messages"][0]["content"], "ping");
        assert_eq!(event["response"]["id"], "chatcmpl-1");
        assert!(event["latency_ms"].is_u64(), "latency_ms is an integer");
        let ts = event["ts"].as_str().expect("ts is a string");
        time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
            .expect("ts is RFC 3339");
    }

    #[test]
    fn unopenable_tape_path_fails_state_construction() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config = config_for(
            "http://127.0.0.1:1",
            &dir.path().join("missing").join("tape.jsonl"),
        );
        let err = AppState::new(&config).expect_err("an unopenable tape must fail");
        assert!(
            matches!(err, AppError::Tape(_)),
            "expected Tape, got {err:?}"
        );
    }

    #[tokio::test]
    async fn non_json_gateway_body_is_taped_as_a_string() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_not_json)))
                .await;
        let (state, tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);

        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        let event: serde_json::Value =
            serde_json::from_str(raw.lines().next().expect("one event per round-trip"))
                .expect("the tape line is valid JSON");
        assert_eq!(event["response"], "gateway replied in plain text");
    }

    #[tokio::test]
    async fn streaming_chat_relays_every_event_in_order_including_done() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (state, _tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(stream_chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("an SSE response sets content-type");
        assert_eq!(content_type, "text/event-stream");
        assert_eq!(
            &body_bytes(response).await[..],
            STREAM_BODY.as_bytes(),
            "every gateway event is relayed in order, [DONE] included"
        );
    }

    #[tokio::test]
    async fn streamed_chat_writes_one_tape_event_with_the_assembled_response() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream)))
                .await;
        let (state, tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(stream_chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let _ = body_bytes(response).await;

        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 1, "exactly one event per streamed chat");
        let event: serde_json::Value =
            serde_json::from_str(lines[0]).expect("the tape line is valid JSON");
        assert_eq!(event["kind"], "chat");
        assert_eq!(event["model"], "test-model");
        assert_eq!(
            event["request"]["stream"], true,
            "the request is taped as received"
        );
        assert_eq!(
            event["response"], "pong",
            "the tape holds the assembled content, not the raw SSE"
        );
        assert!(event["latency_ms"].is_u64(), "latency_ms is an integer");
    }

    #[tokio::test]
    async fn a_mid_stream_gateway_error_is_taped_as_an_error_note() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream_dies)))
                .await;
        let (state, tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(stream_chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_bytes(response).await;
        let text = String::from_utf8(body.to_vec()).expect("the SSE body is UTF-8");
        assert!(
            text.contains("data: {\"choices\":[{\"delta\":{\"content\":\"po\"}}]}"),
            "the good event arrived before the failure: {text:?}"
        );
        assert!(
            !text.contains("[DONE]"),
            "no terminal event after a mid-stream error: {text:?}"
        );

        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 1, "an errored stream still tapes one event");
        let event: serde_json::Value =
            serde_json::from_str(lines[0]).expect("the tape line is valid JSON");
        let message = event["response"]["error"]
            .as_str()
            .expect("the error note is a string");
        assert!(!message.is_empty(), "the error note names the failure");
        assert_eq!(
            event["response"]["content"], "po",
            "the partial content is taped alongside the error"
        );
    }

    #[tokio::test]
    async fn a_declined_stream_is_relayed_and_taped_like_a_buffered_chat() {
        let base_url = spawn_gateway(
            Router::new().route("/v1/chat/completions", post(mock_chat_declines_stream)),
        )
        .await;
        let (state, tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(stream_chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(&body_bytes(response).await[..], UPSTREAM_ERROR.as_bytes());

        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 1, "a declined stream tapes exactly one event");
        let event: serde_json::Value =
            serde_json::from_str(lines[0]).expect("the tape line is valid JSON");
        assert_eq!(event["response"]["error"]["code"], "upstream_unavailable");
    }

    #[tokio::test]
    async fn tape_write_failure_does_not_fail_the_chat_response() {
        struct FailingWriter;
        impl std::io::Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected tape failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let base_url = spawn_mock_gateway().await;
        let gateway = GatewayClient::new(&base_url, "test-key").expect("client builds in tests");
        let state = AppState {
            gateway,
            tape: Arc::new(Tape::with_writer_for_test(FailingWriter)),
        };
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], COMPLETION.as_bytes());
    }
}
