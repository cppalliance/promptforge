//! The axum router, shared handler state, and startup wiring for the
//! workshop server.

use std::sync::Arc;

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{get, post};

use crate::catalog::CatalogBus;
use crate::chat_ws;
use crate::config::{Config, VoiceConfig};
use crate::gateway::{GatewayClient, GatewayError};
use crate::heartbeat::GatewayHealth;
use crate::protocol::Activity;
use crate::push::Push;
use crate::status::StatusBus;
use crate::tape::{Tape, TapeError};
use crate::transcribe::{TranscribeError, VoiceEngine, VoiceSlot};
use crate::voice;
use crate::workspace::{self, Workspace};
use crate::{assets, relay};

/// Address the server binds to when no override is given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7910";

/// Shared handler state: the authenticated gateway client, the session
/// tape, the status bus, and the voice transcription engine slot, filled
/// at startup from local model files or later by the provisioning task.
#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) gateway: GatewayClient,
    pub(crate) tape: Arc<Tape>,
    pub(crate) voice: VoiceSlot,
    pub(crate) status: StatusBus,
    pub(crate) health: GatewayHealth,
    pub(crate) catalog: CatalogBus,
    pub(crate) workspace: Workspace,
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
        let catalog = CatalogBus::new();
        let push = Push::new(status.clone(), catalog.clone());
        // Startup phases are reported as they run; with no client connected
        // yet these land on an empty bus, ready for the first session.
        push.push_status_update(
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
            if let Some(engine) = startup_engine(&config.voice, &push) {
                voice.activate(engine);
            }
        } else if config.voice.enabled() {
            tracing::info!("voice disabled: GPU transcription is unavailable");
            push.push_status_update(
                "Voice disabled",
                "GPU transcription is unavailable; the whisper models stay unloaded",
                Activity::General,
            );
        }
        push.push_idle();
        Ok(Self {
            gateway,
            tape: Arc::new(tape),
            voice,
            status,
            health: GatewayHealth::new(),
            catalog,
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

    /// The status bus, which every `/ws` session subscribes to so it can
    /// forward updates; producers report through [`AppState::push`].
    pub(crate) fn status(&self) -> StatusBus {
        self.status.clone()
    }

    /// The push facade over the status and catalog buses, held by every
    /// subsystem that reports what happened.
    pub(crate) fn push(&self) -> Push {
        Push::new(self.status.clone(), self.catalog.clone())
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
pub(crate) fn startup_engine(config: &VoiceConfig, push: &Push) -> Option<VoiceEngine> {
    if !config.enabled() {
        return None;
    }
    push.push_status_update(
        "Loading whisper model",
        "the interim transcription model",
        Activity::General,
    );
    match VoiceEngine::new(config) {
        Ok(engine) => Some(engine),
        Err(error) => degrade(config, push, &error),
    }
}

/// Maps a startup engine-load failure to its degraded outcome: defer to
/// the provisioning task when the failed model has a source URL, drop an
/// unsourced final pass and run interim-only, or disable voice with an
/// explanation when the interim model can neither load nor be fetched.
fn degrade(config: &VoiceConfig, push: &Push, error: &TranscribeError) -> Option<VoiceEngine> {
    if let TranscribeError::LoadModel { path, .. } = error {
        let sourced = (path == &config.interim_model && !config.interim_source.is_empty())
            || (path == &config.final_model && !config.final_source.is_empty());
        if sourced {
            // The bus is empty at startup and push_idle follows, so the
            // verdict also goes to the log, where it survives.
            tracing::warn!(%error, "voice models not downloaded; deferring to provisioning");
            push.push_status_update(
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
                    push.push_status_update(
                        "Voice final pass unavailable",
                        format!("{error}; takes close with the interim model"),
                        Activity::General,
                    );
                    Some(engine)
                }
                Err(interim_error) => {
                    tracing::warn!(error = %interim_error, "voice disabled at startup");
                    push.push_failure(
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
    push.push_failure("Voice disabled", error.to_string(), Activity::General);
    None
}

/// Returns the workshop server router with every route mounted.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(assets::ui_index))
        .route("/app.js", get(assets::ui_app_js))
        .route("/app.css", get(assets::ui_app_css))
        .route("/style.css", get(assets::ui_style_css))
        .route("/pcm-worklet.js", get(assets::ui_pcm_worklet))
        .route(
            "/icons/promptforge-icon-1.png",
            get(assets::ui_program_icon),
        )
        .route("/health", get(health))
        .route("/v1/models", get(relay::models))
        .route("/profiles", get(relay::profiles))
        .route("/profiles/switch", post(relay::switch_profile))
        .route("/chat", post(relay::chat))
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

/// Shared fixtures for the router tests here and in [`crate::assets`] and
/// [`crate::relay`]: state construction against a stub gateway address and
/// the small helpers every route test leans on. [`fixtures::spawn_gateway`]
/// is additionally re-exported to the integration-test binary through the
/// `test-fixtures` feature the crate's own dev-dependency enables.
// An `allow` rather than an `expect`: whether the lint fires here depends
// on the build's cfg permutation (clippy suppresses expect_used inside
// test-cfg'd code on its own), so an expectation would be unfulfilled in
// some builds and fail the -D warnings gate.
#[cfg(any(test, feature = "test-fixtures"))]
#[allow(
    clippy::expect_used,
    reason = "test fixtures fail by panicking with the invariant named"
)]
pub(crate) mod fixtures {
    #[cfg(test)]
    use std::path::Path;

    use axum::Router;
    #[cfg(test)]
    use axum::body::to_bytes;
    #[cfg(test)]
    use axum::response::Response;

    #[cfg(test)]
    use crate::app::AppState;
    #[cfg(test)]
    use crate::config::{Config, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig};

    /// Builds a configuration pointing at `base_url`, taping to `tape_path`.
    #[cfg(test)]
    pub(crate) fn config_for(base_url: &str, tape_path: &Path) -> Config {
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
    #[cfg(test)]
    pub(crate) fn state_for(base_url: &str) -> (AppState, tempfile::TempDir) {
        let tape_dir = tempfile::TempDir::new().expect("tempdir");
        let config = config_for(base_url, &tape_dir.path().join("tape.jsonl"));
        let state = AppState::new(&config).expect("state builds in tests");
        (state, tape_dir)
    }

    /// Collects a response body already buffered in memory.
    #[cfg(test)]
    pub(crate) async fn body_bytes(response: Response) -> axum::body::Bytes {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is in memory already")
    }

    /// Binds `app` as a mock gateway on a free loopback port and returns its
    /// base URL.
    ///
    /// # Panics
    /// Panics when the loopback bind fails or the bound address cannot be
    /// read.
    pub async fn spawn_gateway(app: Router) -> String {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode};
    use axum::response::Response;
    use tower::ServiceExt;

    use super::fixtures::{body_bytes, config_for, spawn_gateway, state_for};
    use crate::protocol::{Severity, StatusBarUpdate};
    use crate::transcribe::fixtures;

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

    #[test]
    fn default_bind_is_loopback_port_7910() {
        assert_eq!(DEFAULT_ADDR, "127.0.0.1:7910");
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
        let push = Push::new(status, CatalogBus::new());
        let config = VoiceConfig {
            interim_model: PathBuf::from("definitely-missing-model.bin"),
            ..VoiceConfig::default()
        };
        let engine = startup_engine(&config, &push);
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
        let push = Push::new(status, CatalogBus::new());
        let config = VoiceConfig {
            interim_model: PathBuf::from("definitely-missing-model.bin"),
            interim_source: "https://example.com/ggml.bin".to_string(),
            ..VoiceConfig::default()
        };
        let engine = startup_engine(&config, &push);
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
        let push = Push::new(status, CatalogBus::new());
        let config = VoiceConfig {
            interim_model: fixtures::require_model(),
            final_model: PathBuf::from("definitely-missing-final-model.bin"),
            ..VoiceConfig::default()
        };
        let engine = startup_engine(&config, &push);
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
        let push = Push::new(status, CatalogBus::new());
        let config = VoiceConfig {
            interim_model: PathBuf::from("model.bin"),
            window_seconds: 0,
            ..VoiceConfig::default()
        };
        let engine = startup_engine(&config, &push);
        assert!(engine.is_none(), "invalid tuning costs voice, not startup");
        let verdict = degradation(&mut rx);
        assert_eq!(verdict.label, "Voice disabled");
        assert!(
            verdict.description.contains("window_seconds"),
            "the explanation names the bad field: {verdict:?}"
        );
    }
}
