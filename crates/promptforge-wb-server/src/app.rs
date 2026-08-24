//! The axum router, handlers, and serving loop for the workbench server.

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use crate::config::Config;
use crate::gateway::{ChatRequest, GatewayClient, GatewayError, GatewayResponse};
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

/// Relays the gateway's model catalog to the caller verbatim.
async fn models(State(state): State<AppState>) -> Response {
    relay(state.gateway.list_models().await)
}

/// Forwards a chat completion to the gateway, tapes the round-trip, and
/// relays the reply verbatim.
///
/// A completed round-trip is recorded on the session tape; a tape failure is
/// logged and never changes the response.
async fn chat(State(state): State<AppState>, body: String) -> Response {
    let request_value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => return bad_request(&error),
    };
    let request: ChatRequest = match serde_json::from_value(request_value.clone()) {
        Ok(request) => request,
        Err(error) => return bad_request(&error),
    };
    let started = Instant::now();
    let result = state.gateway.chat_completion(&request).await;
    let latency = started.elapsed();
    if let Ok(upstream) = &result {
        let response_value = serde_json::from_slice(&upstream.body).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&upstream.body).into_owned())
        });
        let ChatRequest { model, .. } = request;
        let tape = Arc::clone(&state.tape);
        let written = tokio::task::spawn_blocking(move || {
            let event = TapeEvent::chat(model, request_value, response_value, latency)?;
            tape.record(&event)
        })
        .await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(%error, "session tape event was not recorded"),
            Err(error) => tracing::error!(%error, "session tape writer did not finish"),
        }
    }
    relay(result)
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

    #[test]
    fn default_bind_is_loopback_port_7910() {
        assert_eq!(DEFAULT_ADDR, "127.0.0.1:7910");
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
