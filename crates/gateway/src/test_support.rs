//! Shared admin-route test harness: serves `build_router` over a state
//! assembled from one fixture profile.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gateway_config::Config;
use shared_progress::ProgressHub;

use crate::routing::Routing;
use crate::{AppState, ProfileSelection, build_router};

/// Filesystem context for shadow-write and env admin-route tests.
pub(crate) struct AdminPaths {
    /// Fixture directory used by filesystem-boundary tests.
    pub(crate) fixture_dir: PathBuf,
    /// The active profile name.
    pub(crate) active: String,
    /// The single config path.
    pub(crate) config_path: PathBuf,
}

/// Serves `build_router` over a state assembled from `config` with no
/// running children: the retained config still carries everything the
/// admin routes read (the cache root, the `[[local_model]]` entries).
pub(crate) async fn serve(config: Config) -> SocketAddr {
    serve_with(config, None, None).await
}

/// Serves like [`serve`], but with the Hugging Face proxy replaced, so a
/// test can aim the `/admin/hf/*` routes at a local stub hub with an
/// explicit token instead of the process env.
pub(crate) async fn serve_with_hf(config: Config, hf: crate::hf::HfProxy) -> SocketAddr {
    serve_with(config, Some(hf), None).await
}

/// Serves like [`serve`], but with active profile and config-file context.
pub(crate) async fn serve_with_paths(config: Config, paths: AdminPaths) -> SocketAddr {
    serve_with(config, None, Some(paths)).await
}

/// The shared harness body behind [`serve`], [`serve_with_hf`], and
/// [`serve_with_paths`].
async fn serve_with(
    config: Config,
    hf: Option<crate::hf::HfProxy>,
    paths: Option<AdminPaths>,
) -> SocketAddr {
    let mut state = app_state(config, paths);
    if let Some(hf) = hf {
        state.hf = Arc::new(hf);
    }
    serve_state(state).await
}

/// Builds the state the harness serves, so a test can override an
/// injected collaborator (the HF proxy, the reveal launcher) or drive a
/// handler directly.
pub(crate) fn app_state(config: Config, paths: Option<AdminPaths>) -> AppState {
    let routing = Routing::from_config(&config).expect("routing builds");
    state_over(config, routing, paths)
}

/// Builds state with deterministic speech workers for Gateway route tests.
#[cfg(feature = "stt")]
pub(crate) fn app_state_with_scripted_stt(
    config: Config,
    factory: gateway_stt::test_fixtures::ScriptedModelFactory,
) -> Result<AppState, String> {
    let service = gateway_stt::test_fixtures::scripted_service(factory, 15, 500)
        .map_err(|error| error.to_string())?;
    let mut state = app_state(config, None);
    state.speech = service;
    Ok(state)
}

/// Builds the state the instant-ready boot path serves: an empty routing
/// table over `config`, no active profile, nothing local running - the
/// shell the boot `LoadProfile` command fills.
pub(crate) fn boot_state(config: Config) -> AppState {
    state_over(config, Routing::empty(), None)
}

/// [`boot_state`] with config-file context, so a boot command's switch can
/// persist the active-profile selection.
pub(crate) fn boot_state_with_paths(config: Config, paths: AdminPaths) -> AppState {
    state_over(config, Routing::empty(), Some(paths))
}

/// Arms the state's empty speech facade to publish `factory`'s
/// deterministic workers on its one initial load, replacing the Whisper
/// backend for boot-path tests.
#[cfg(feature = "stt")]
pub(crate) fn arm_boot_speech(
    state: &mut AppState,
    factory: impl gateway_stt_engine::ModelFactory,
) {
    let policy = gateway_stt_engine::EnginePolicy::new(15, 500, false)
        .expect("the scripted boot policy is valid");
    state.speech = gateway_stt::SpeechService::new().with_scripted_initial_load(factory, policy);
}

/// A scripted speech factory whose worker construction parks until
/// [`GatedConstructionFactory::release`], so a test can hold the boot
/// command inside its STT load while it probes the serving surface.
#[cfg(feature = "stt")]
#[derive(Clone, Debug)]
pub(crate) struct GatedConstructionFactory {
    inner: Arc<gateway_stt::test_fixtures::ScriptedModelFactory>,
    entered: Arc<std::sync::atomic::AtomicBool>,
    open: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

#[cfg(feature = "stt")]
impl GatedConstructionFactory {
    /// Wraps a scripted factory with a closed construction gate.
    pub(crate) fn new(inner: gateway_stt::test_fixtures::ScriptedModelFactory) -> Self {
        Self {
            inner: Arc::new(inner),
            entered: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            open: Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new())),
        }
    }

    /// Whether worker construction has entered the gate.
    pub(crate) fn entered(&self) -> bool {
        self.entered.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Lets parked worker construction complete.
    pub(crate) fn release(&self) {
        let (lock, changed) = &*self.open;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        changed.notify_all();
    }
}

#[cfg(feature = "stt")]
impl gateway_stt_engine::ModelFactory for GatedConstructionFactory {
    fn create(
        &self,
        mode: gateway_stt_engine::DecodeMode,
    ) -> Result<Option<Box<dyn gateway_stt_engine::Decoder>>, gateway_stt_engine::TranscribeError>
    {
        self.entered
            .store(true, std::sync::atomic::Ordering::Release);
        let (lock, changed) = &*self.open;
        let mut open = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*open {
            open = changed
                .wait(open)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(open);
        self.inner.create(mode)
    }
}

/// The shared body behind [`app_state`] and [`boot_state`].
fn state_over(config: Config, routing: Routing, paths: Option<AdminPaths>) -> AppState {
    let key = config.server_key();
    let config = Arc::new(config);
    let (config_path, selection) = match paths {
        Some(paths) => (
            Some(paths.config_path),
            ProfileSelection {
                name: Some(paths.active),
                model_allowlist: None,
            },
        ),
        None => (None, ProfileSelection::default()),
    };
    AppState::from_parts(
        Arc::new(routing),
        key,
        Arc::clone(&config),
        #[cfg(feature = "local")]
        crate::local::LocalRuntime::empty(),
        #[cfg(feature = "stt")]
        gateway_stt::SpeechService::new(),
        #[cfg(feature = "web-search")]
        config.web_search_config(),
        config_path,
        selection,
        Arc::new(ProgressHub::new()),
    )
}

/// Binds an ephemeral loopback listener and serves `state` on it,
/// returning the bound address. Connect info and the host-authority wall
/// are wired exactly as in the production serve path, so loopback-only
/// routes see a peer address and the wall sees the bound socket.
pub(crate) async fn serve_state(state: AppState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test listener binds");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(
            listener,
            build_router(state, Some(addr)).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    addr
}

#[cfg(all(test, feature = "stt"))]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};
    use tower::ServiceExt;

    use super::*;

    fn transcription_body() -> (String, Vec<u8>) {
        const BOUNDARY: &str = "scripted-stt-boundary";
        let mut wav = vec![
            b'R', b'I', b'F', b'F', 38, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
            16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0, b'd', b'a',
            b't', b'a', 2, 0, 0, 0, 0, 32,
        ];
        let mut body = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n\
             scripted-interim\r\n\
             --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"sample.wav\"\r\n\
             Content-Type: audio/wav\r\n\r\n"
        )
        .into_bytes();
        body.append(&mut wav);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        (BOUNDARY.to_owned(), body)
    }

    #[tokio::test]
    async fn scripted_workers_can_be_injected_without_a_production_constructor() {
        const TRANSCRIPT: &str = "gateway scripted route sentinel";

        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
        )
        .expect("config parses");
        let decoder = ScriptedDecoder::new();
        decoder.push_text(TRANSCRIPT);
        let state = app_state_with_scripted_stt(config, ScriptedModelFactory::new(decoder.clone()))
            .expect("scripted state builds");
        let (boundary, body) = transcription_body();

        let response = build_router(state, None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header("authorization", "Bearer test-token")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads");
        let response: serde_json::Value =
            serde_json::from_slice(&body).expect("response body is JSON");
        assert_eq!(response["text"], TRANSCRIPT);
        let requests = decoder.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].mode(),
            gateway_stt::test_fixtures::DecodeMode::Interim
        );
        assert_eq!(requests[0].samples(), &[0.25]);
        assert!(requests[0].guidance().is_empty());
        assert!(requests[0].finalized().is_empty());
    }

    #[tokio::test]
    async fn batch_inference_preserves_the_gateway_error_message_contract() {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
        )
        .expect("config parses");
        let decoder = ScriptedDecoder::new();
        decoder.push_error("scripted inference sentinel");
        let state = app_state_with_scripted_stt(config, ScriptedModelFactory::new(decoder))
            .expect("scripted state builds");
        let (boundary, body) = transcription_body();

        let response = build_router(state, None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header("authorization", "Bearer test-token")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads");
        let response: serde_json::Value =
            serde_json::from_slice(&body).expect("response body is JSON");
        assert_eq!(
            response,
            serde_json::json!({
                "error": {
                    "message": "transcription failed",
                    "type": "server_error",
                    "code": "transcription_error",
                }
            })
        );
    }

    async fn get_json(state: AppState, uri: &'static str) -> serde_json::Value {
        let response = build_router(state, None)
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads");
        serde_json::from_slice(&body).expect("response body is JSON")
    }

    #[tokio::test]
    async fn ready_scripted_pair_is_published_through_gateway_surfaces() {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
        )
        .expect("config parses");
        let factory = ScriptedModelFactory::new(ScriptedDecoder::new())
            .with_final(ScriptedDecoder::new())
            .with_gpu_available(true);
        let state = app_state_with_scripted_stt(config, factory).expect("scripted state builds");
        let service = state.speech.clone();

        let status = get_json(state.clone(), "/admin/status").await;
        assert_eq!(
            status["speech"],
            serde_json::json!({
                "configured": true,
                "ready": true,
                "gpu": true,
            })
        );
        let speech_endpoint = status["endpoints"]
            .as_array()
            .expect("endpoints are an array")
            .iter()
            .find(|entry| entry["path"] == "/v1/audio/transcriptions")
            .expect("speech endpoint is present");
        assert_eq!(speech_endpoint["ready"], true);
        assert_eq!(speech_endpoint["provisioning"], false);

        let catalog = get_json(state, "/v1/models").await;
        assert_eq!(
            catalog["data"]
                .as_array()
                .expect("catalog data")
                .iter()
                .map(|model| model["id"].as_str().expect("model id"))
                .collect::<Vec<_>>(),
            ["scripted-interim", "scripted-final", "realtime-transcribe"]
        );
        service.shutdown();
    }
}
