//! The axum router, handlers, and shared state for the workshop server.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use crate::catalog::CatalogBus;
use crate::chat_ws;
use crate::config::{Config, VoiceConfig};
use crate::gateway::{GatewayClient, GatewayError, GatewayResponse};
use crate::heartbeat::GatewayHealth;
use crate::protocol::{Activity, ChatRequest};
use crate::status::StatusBus;
use crate::tape::{Tape, TapeError, TapeEvent};
use crate::transcribe::{TranscribeError, VoiceEngine, VoiceSlot};
use crate::voice;
use crate::workspace::{self, Workspace};

/// Address the server binds to when no override is given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7910";

/// Shared handler state: the authenticated gateway client, the session
/// tape, the status bus, and the voice transcription engine slot, filled
/// at startup from local model files or later by the provisioning task.
#[derive(Debug, Clone)]
pub struct AppState {
    gateway: GatewayClient,
    tape: Arc<Tape>,
    voice: VoiceSlot,
    status: StatusBus,
    health: GatewayHealth,
    catalog: CatalogBus,
    workspace: Workspace,
}

impl AppState {
    /// Builds shared state from the loaded configuration.
    ///
    /// When `[voice]` names an interim model whose file exists and GPU
    /// transcription is available, the engine loads here. A configured
    /// model that is missing or unloadable never fails startup: when the
    /// model has a source URL, activation defers to the provisioning task
    /// (which fetches it through the gateway cache); otherwise voice
    /// degrades to disabled with a status-bar explanation. Without GPU
    /// transcription the models are never loaded at all.
    ///
    /// # Errors
    /// Returns [`AppError::Gateway`] if the HTTP client cannot be built and
    /// [`AppError::Tape`] if the session tape cannot be opened.
    pub fn new(config: &Config) -> Result<Self, AppError> {
        let status = StatusBus::new();
        // Startup phases are reported as they run; with no client connected
        // yet these land on an empty bus, ready for the first session.
        status.info(
            "Connecting to gateway",
            format!("base URL {}", config.gateway.base_url),
            Activity::General,
        );
        let gateway = GatewayClient::new(&config.gateway.base_url, &config.gateway.api_key)
            .map_err(AppError::Gateway)?;
        let tape = Tape::open(&config.tape.path).map_err(AppError::Tape)?;
        let voice = VoiceSlot::default();
        // Voice is GPU-only: without the CUDA backend and an NVIDIA driver
        // a take stalls on a CPU pass and the UI hides the mic, so the
        // server never loads the multi-gigabyte whisper models it could
        // not use, and never announces voice over a mic that is not there.
        if crate::transcribe::gpu_transcription_available() {
            if let Some(engine) = startup_engine(&config.voice, &status) {
                voice.activate(engine);
            }
        } else if config.voice.enabled() {
            tracing::info!("voice disabled: GPU transcription is unavailable");
            status.info(
                "Voice disabled",
                "GPU transcription is unavailable; the whisper models stay unloaded",
                Activity::General,
            );
        }
        status.idle();
        Ok(Self {
            gateway,
            tape: Arc::new(tape),
            voice,
            status,
            health: GatewayHealth::new(),
            catalog: CatalogBus::new(),
            workspace: Workspace::new(),
        })
    }

    /// The voice transcription engine, when one has loaded.
    pub(crate) fn voice_engine(&self) -> Option<Arc<VoiceEngine>> {
        self.voice.engine()
    }

    /// The voice engine slot, shared with the provisioning task, which
    /// fills it once the gateway cache has provided the models.
    pub(crate) fn voice_slot(&self) -> VoiceSlot {
        self.voice.clone()
    }

    /// The status bus, shared with every subsystem that reports what it is
    /// doing.
    pub(crate) fn status(&self) -> StatusBus {
        self.status.clone()
    }

    /// The gateway client, shared with the chat WebSocket sessions.
    pub(crate) fn gateway_client(&self) -> &GatewayClient {
        &self.gateway
    }

    /// The session tape, shared with the chat WebSocket sessions.
    pub(crate) fn tape(&self) -> &Arc<Tape> {
        &self.tape
    }

    /// Shared gateway reachability, published by the heartbeat; the
    /// gateway-dependent routes read it to short-circuit while the gateway
    /// is down.
    pub(crate) fn health(&self) -> &GatewayHealth {
        &self.health
    }

    /// The catalog bus, which the heartbeat publishes the refreshed model
    /// catalog to on a gateway reconnect and every `/ws` session forwards
    /// from.
    pub(crate) fn catalog(&self) -> CatalogBus {
        self.catalog.clone()
    }

    /// The confined workspace, shared with the `/workspace/*` handlers;
    /// grants registered through `POST /workspace/grant` are visible to
    /// every clone immediately.
    pub(crate) fn workspace(&self) -> &Workspace {
        &self.workspace
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

/// Builds the startup voice engine from local model files only.
///
/// Returns `None` when voice is unconfigured, when a missing model has a
/// source URL (the provisioning task fetches and activates it once the
/// gateway answers), or when voice has degraded to disabled with a
/// status-bar explanation. Never fails: a bad model path or invalid
/// `[voice]` tuning costs voice, not startup.
pub(crate) fn startup_engine(config: &VoiceConfig, status: &StatusBus) -> Option<VoiceEngine> {
    if !config.enabled() {
        return None;
    }
    status.info(
        "Loading whisper model",
        "the interim transcription model",
        Activity::General,
    );
    match VoiceEngine::new(config) {
        Ok(engine) => Some(engine),
        Err(error) => degrade(config, status, &error),
    }
}

/// Maps a startup engine-load failure to its degraded outcome: defer to
/// the provisioning task when the failed model has a source URL, drop an
/// unsourced final pass and run interim-only, or disable voice with an
/// explanation when the interim model can neither load nor be fetched.
fn degrade(
    config: &VoiceConfig,
    status: &StatusBus,
    error: &TranscribeError,
) -> Option<VoiceEngine> {
    if let TranscribeError::LoadModel { path, .. } = error {
        let sourced = (path == &config.interim_model && !config.interim_source.is_empty())
            || (path == &config.final_model && !config.final_source.is_empty());
        if sourced {
            // The bus is empty at startup and idle() follows, so the
            // verdict also goes to the log, where it survives.
            tracing::warn!(%error, "voice models not downloaded; deferring to provisioning");
            status.info(
                "Voice models not downloaded",
                format!("{error}; the gateway cache provides them once connected"),
                Activity::General,
            );
            return None;
        }
        if path == &config.final_model {
            // The final pass is optional: an unsourced missing final model
            // drops to interim-only rather than costing voice entirely.
            let mut interim_only = config.clone();
            interim_only.final_model = std::path::PathBuf::new();
            return match VoiceEngine::new(&interim_only) {
                Ok(engine) => {
                    tracing::warn!(%error, "voice final pass unavailable; running interim-only");
                    status.info(
                        "Voice final pass unavailable",
                        format!("{error}; takes close with the interim model"),
                        Activity::General,
                    );
                    Some(engine)
                }
                Err(interim_error) => {
                    tracing::warn!(error = %interim_error, "voice disabled at startup");
                    status.error(
                        "Voice disabled",
                        interim_error.to_string(),
                        Activity::General,
                    );
                    None
                }
            };
        }
    }
    tracing::warn!(%error, "voice disabled at startup");
    status.error("Voice disabled", error.to_string(), Activity::General);
    None
}

/// Returns the workshop server router with every route mounted.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(ui_index))
        .route("/app.js", get(ui_app_js))
        .route("/app.css", get(ui_app_css))
        .route("/style.css", get(ui_style_css))
        .route("/pcm-worklet.js", get(ui_pcm_worklet))
        .route("/icons/promptforge-icon-1.png", get(ui_program_icon))
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/chat", post(chat))
        .route("/ws", get(chat_ws::upgrade))
        .route("/voice", get(voice::upgrade))
        .route("/voice/capability", get(voice_capability))
        .route("/workspace/tree", get(workspace::tree))
        .route(
            "/workspace/file",
            get(workspace::read_file).put(workspace::write_file),
        )
        .route("/workspace/grant", post(workspace::grant))
        .with_state(state)
}

/// Answers the health probe with a static JSON body.
async fn health() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"status":"serving"}"#,
    )
}

/// Reports whether voice transcription can run on the GPU, so the UI can
/// hide the mic rather than offer a take that stalls on a CPU pass.
async fn voice_capability() -> impl IntoResponse {
    let gpu = crate::transcribe::gpu_transcription_available();
    (
        [(header::CONTENT_TYPE, "application/json")],
        format!(r#"{{"gpu":{gpu}}}"#),
    )
}

/// The workshop UI assets under `ui/dist/`, written by the crate's build
/// script (the esbuild bundle plus copies of the static files). Debug builds
/// read the files from disk at request time, so UI edits need no Rust
/// recompile; release builds embed them into the binary.
#[derive(rust_embed::Embed)]
#[folder = "ui/dist/"]
struct UiAssets;

/// Serves one UI asset from [`UiAssets`] with the given content type.
fn ui_asset(path: &str, content_type: &'static str) -> Response {
    match UiAssets::get(path) {
        Some(asset) => (
            [(header::CONTENT_TYPE, content_type)],
            asset.data.into_owned(),
        )
            .into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("ui asset not found: {path}"),
        )
            .into_response(),
    }
}

/// Serves the chat UI's `index.html`.
async fn ui_index() -> Response {
    ui_asset("index.html", "text/html; charset=utf-8")
}

/// Serves the chat UI's bundled application script.
async fn ui_app_js() -> Response {
    ui_asset("app.js", "text/javascript; charset=utf-8")
}

/// Serves the stylesheet esbuild extracts from the bundle's CSS imports
/// (the vendored murm-ui and dockview styles).
async fn ui_app_css() -> Response {
    ui_asset("app.css", "text/css; charset=utf-8")
}

/// Serves the chat UI's own stylesheet.
async fn ui_style_css() -> Response {
    ui_asset("style.css", "text/css; charset=utf-8")
}

/// Serves the AudioWorklet PCM capture processor.
async fn ui_pcm_worklet() -> Response {
    ui_asset("pcm-worklet.js", "text/javascript; charset=utf-8")
}

/// Serves the program icon shown in the custom title bar (the cold
/// medallion frame; the heat stages are reserved for a future activity
/// animation).
async fn ui_program_icon() -> Response {
    ui_asset("icons/promptforge-icon-1.png", "image/png")
}

/// Relays the gateway's model catalog to the caller verbatim.
///
/// While the heartbeat reports the gateway down, the catalog is not
/// attempted: the route answers 502 with a user-visible message instead.
async fn models(State(state): State<AppState>) -> Response {
    if !state.health().is_reachable() {
        return gateway_unreachable();
    }
    let status = state.status();
    status.info(
        "Loading models...",
        "fetching the gateway model catalog",
        Activity::General,
    );
    let result = state.gateway.list_models().await;
    report_gateway_outcome(&status, &result, "GET /v1/models");
    relay(result)
}

/// Reports a gateway call's outcome on the status bus: back to idle on
/// success, otherwise the error label matching the failure shape.
fn report_gateway_outcome(
    status: &StatusBus,
    result: &Result<GatewayResponse, GatewayError>,
    route: &str,
) {
    match result {
        Ok(upstream) if upstream.status.is_success() => status.idle(),
        Ok(upstream) => status.error(
            format!("Gateway error: {}", upstream.status),
            format!("{route} answered a non-success status"),
            Activity::General,
        ),
        Err(error) => status.error("Connection lost", error.to_string(), Activity::General),
    }
}

/// Forwards a buffered chat completion to the gateway, tapes the
/// round-trip, and relays the reply verbatim.
///
/// A completed round-trip is recorded on the session tape; a tape failure is
/// logged and never changes the response. Streaming moved to `GET /ws`: a
/// request carrying `"stream": true` is rejected with 400.
async fn chat(State(state): State<AppState>, body: String) -> Response {
    let request_value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => return bad_request(&error),
    };
    if request_value
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return stream_unsupported();
    }
    let request: ChatRequest = match serde_json::from_value(request_value.clone()) {
        Ok(request) => request,
        Err(error) => return bad_request(&error),
    };
    // A gateway the heartbeat knows is down is not attempted, matching the
    // /ws chat short-circuit.
    if !state.health().is_reachable() {
        return gateway_unreachable();
    }
    let status = state.status();
    status.info(
        "Submitting request...",
        "a buffered chat completion",
        Activity::General,
    );
    status.info(
        "Waiting for response...",
        "the gateway has the request",
        Activity::General,
    );
    let started = Instant::now();
    let result = state.gateway.chat_completion(&request).await;
    report_gateway_outcome(&status, &result, "POST /v1/chat/completions");
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

/// Renders the 502 envelope for a gateway the heartbeat knows is down: the
/// request is not attempted, and the message is user-visible.
fn gateway_unreachable() -> Response {
    (
        axum::http::StatusCode::BAD_GATEWAY,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "error": {
                "message": "Gateway unreachable",
                "code": "gateway_unreachable",
            }
        })
        .to_string(),
    )
        .into_response()
}

/// Renders the 400 envelope for a chat request that asked for a stream.
fn stream_unsupported() -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "error": {
                "message": "streaming moved to GET /ws; POST /chat is buffered only",
                "code": "stream_unsupported",
            }
        })
        .to_string(),
    )
        .into_response()
}

/// Parses a gateway body as JSON, falling back to a plain string.
pub(crate) fn value_from_bytes(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(body).into_owned()))
}

/// Records one chat round-trip on the session tape.
///
/// A tape failure is logged and never changes the response.
pub(crate) async fn tape_round_trip(
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

/// Turns a gateway call outcome into the workshop's HTTP response.
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

    use std::path::{Path, PathBuf};

    use axum::Json;
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderMap, Request, StatusCode};
    use tower::ServiceExt;

    use crate::config::{GatewayConfig, ServerConfig, TapeConfig, VoiceConfig};
    use crate::protocol::{Severity, StatusBarUpdate};
    use crate::transcribe::fixtures;

    const CATALOG: &str = r#"{"object":"list","data":[{"id":"test-model","object":"model","created":1,"owned_by":"promptforge"}]}"#;
    const COMPLETION: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
    const UPSTREAM_ERROR: &str =
        r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;
    const CHAT_BODY: &str =
        r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}]}"#;
    const STREAM_CHAT_BODY: &str =
        r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}],"stream":true}"#;

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
            voice: VoiceConfig::default(),
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

    /// Reports whether the request carried an `Authorization` header, so
    /// the client tests can observe what was sent.
    async fn mock_auth_probe(headers: HeaderMap) -> Response {
        let body = if headers.contains_key(header::AUTHORIZATION) {
            "auth"
        } else {
            "no-auth"
        };
        ([(header::CONTENT_TYPE, "text/plain")], body).into_response()
    }

    #[tokio::test]
    async fn empty_api_key_sends_no_authorization_header() {
        let base_url = spawn_gateway(Router::new().route("/v1/models", get(mock_auth_probe))).await;
        let anonymous = GatewayClient::new(&base_url, "").expect("client builds");
        let response = anonymous.list_models().await.expect("request completes");
        assert_eq!(response.body, b"no-auth", "empty key sends no header");

        let keyed = GatewayClient::new(&base_url, "test-key").expect("client builds");
        let response = keyed.list_models().await.expect("request completes");
        assert_eq!(response.body, b"auth", "a set key still authenticates");
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
    async fn bundled_app_css_is_served_as_css() {
        assert_ui_asset("/app.css", "text/css; charset=utf-8").await;
    }

    #[tokio::test]
    async fn pcm_worklet_is_served_as_javascript() {
        assert_ui_asset("/pcm-worklet.js", "text/javascript; charset=utf-8").await;
    }

    #[tokio::test]
    async fn program_icon_is_served_as_png() {
        assert_ui_asset("/icons/promptforge-icon-1.png", "image/png").await;
    }

    /// A plain GET to `/ws` without upgrade headers is rejected with 400,
    /// which proves the route is mounted; the WebSocket chat flow is covered
    /// by the `chat_ws` module's own tests over a live socket.
    #[tokio::test]
    async fn ws_route_rejects_a_non_upgrade_get() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/ws")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// A plain GET to `/voice` without upgrade headers is rejected with 400,
    /// which proves the route is mounted; the full WebSocket session flow is
    /// covered by the `voice` module's own tests over a live socket.
    #[tokio::test]
    async fn voice_route_rejects_a_non_upgrade_get() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/voice")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn voice_capability_reports_the_build() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/voice/capability")
            .body(Body::empty())
            .expect("request builds");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the route answers");
        assert_eq!(response.status(), StatusCode::OK);
        let expected = crate::transcribe::gpu_transcription_available();
        assert_eq!(
            &body_bytes(response).await[..],
            format!(r#"{{"gpu":{expected}}}"#).as_bytes()
        );
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
    async fn a_gateway_known_down_short_circuits_the_catalog_with_bad_gateway() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        state.health().publish(false);
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
        assert_eq!(
            json["error"]["message"], "Gateway unreachable",
            "the short-circuit message is user-visible"
        );
    }

    #[tokio::test]
    async fn a_gateway_known_down_short_circuits_buffered_chat_with_bad_gateway() {
        let (state, tape_dir) = state_for("http://127.0.0.1:1");
        state.health().publish(false);
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "gateway_unreachable");
        assert_eq!(json["error"]["message"], "Gateway unreachable");
        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        assert!(
            raw.trim().is_empty(),
            "no upstream attempt means no tape event"
        );
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

    /// Drains the startup phase frames emitted before the degradation
    /// verdict and returns the verdict frame.
    fn degradation(rx: &mut tokio::sync::broadcast::Receiver<StatusBarUpdate>) -> StatusBarUpdate {
        // The first frame is the "Loading whisper model" phase note; the
        // verdict follows it.
        rx.try_recv().expect("the loading phase is reported");
        rx.try_recv().expect("the degradation verdict is reported")
    }

    #[test]
    fn a_missing_interim_model_with_no_source_degrades_to_disabled_voice() {
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let config = VoiceConfig {
            interim_model: PathBuf::from("definitely-missing-model.bin"),
            ..VoiceConfig::default()
        };
        let engine = startup_engine(&config, &status);
        assert!(engine.is_none(), "voice degrades to disabled, not fatal");
        let verdict = degradation(&mut rx);
        assert_eq!(verdict.label, "Voice disabled");
        assert_eq!(verdict.severity, Severity::Error);
        assert!(
            verdict.description.contains("definitely-missing-model.bin"),
            "the explanation names the missing path: {verdict:?}"
        );
    }

    #[test]
    fn a_missing_model_with_a_source_defers_to_provisioning() {
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let config = VoiceConfig {
            interim_model: PathBuf::from("definitely-missing-model.bin"),
            interim_source: "https://example.com/ggml.bin".to_string(),
            ..VoiceConfig::default()
        };
        let engine = startup_engine(&config, &status);
        assert!(engine.is_none(), "the engine activates later, not now");
        let verdict = degradation(&mut rx);
        assert_eq!(verdict.label, "Voice models not downloaded");
        assert_eq!(verdict.severity, Severity::Info);
        assert_eq!(verdict.activity, Activity::General);
    }

    #[test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    fn a_missing_unsourced_final_model_drops_the_final_pass() {
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: PathBuf::from("definitely-missing-final-model.bin"),
            ..VoiceConfig::default()
        };
        let engine = startup_engine(&config, &status);
        let engine = engine.expect("the interim model still loads");
        assert!(
            engine.final_pass_absent_for_test(),
            "the final pass was dropped"
        );
        let verdict = degradation(&mut rx);
        assert_eq!(verdict.label, "Voice final pass unavailable");
        assert_eq!(verdict.severity, Severity::Info);
    }

    #[test]
    fn invalid_voice_tuning_degrades_instead_of_failing_startup() {
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let config = VoiceConfig {
            interim_model: PathBuf::from("model.bin"),
            window_seconds: 0,
            ..VoiceConfig::default()
        };
        let engine = startup_engine(&config, &status);
        assert!(engine.is_none(), "invalid tuning costs voice, not startup");
        let verdict = degradation(&mut rx);
        assert_eq!(verdict.label, "Voice disabled");
        assert!(
            verdict.description.contains("window_seconds"),
            "the explanation names the bad field: {verdict:?}"
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
    async fn a_streaming_chat_request_is_rejected_with_bad_request() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let response = router(state)
            .oneshot(stream_chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "stream_unsupported");
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
            voice: VoiceSlot::default(),
            status: StatusBus::new(),
            health: GatewayHealth::new(),
            catalog: CatalogBus::new(),
            workspace: Workspace::new(),
        };
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], COMPLETION.as_bytes());
    }
}
