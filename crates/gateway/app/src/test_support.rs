//! Shared admin-route test harness: serves `build_router` over a state
//! assembled from one fixture profile.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gateway_config::Config;
use gateway_progress::ProgressHub;

use crate::error::GatewayError;
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

/// A tempdir-backed state with a real config file and a `local` cache
/// root, so every walled handler has something to answer with once past
/// the wall. The Hugging Face proxy points at a dead loopback port, so a
/// sweep that reaches the HF routes fails its connection rather than
/// calling the real hub.
pub(crate) fn walled_fixture() -> (tempfile::TempDir, AppState) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let models = temp.path().join("cache").join("models");
    std::fs::create_dir_all(&models).expect("mkdir cache models");
    let boot = temp.path().join("gateway.toml");
    std::fs::write(&boot, "").expect("write boot");
    let config = Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = '{cache}'
"#,
        cache = temp.path().join("cache").display(),
    ))
    .expect("the fixture profile parses");
    let mut state = app_state(
        config,
        Some(AdminPaths {
            fixture_dir: temp.path().to_path_buf(),
            active: "main".to_owned(),
            config_path: boot,
        }),
    );
    state.hf = Arc::new(crate::admin::walled::hf::HfProxy::new(
        "http://127.0.0.1:9".to_owned(),
        None,
    ));
    (temp, state)
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
pub(crate) async fn serve_with_hf(
    config: Config,
    hf: crate::admin::walled::hf::HfProxy,
) -> SocketAddr {
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
    hf: Option<crate::admin::walled::hf::HfProxy>,
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
) -> anyhow::Result<AppState> {
    let service = gateway_stt::test_fixtures::scripted_service(factory, 15, 500)?;
    let mut state = app_state(config, None);
    state.speech = service;
    Ok(state)
}

/// Builds the state the instant-ready boot path serves: the remote routing
/// table over `config`, no active profile, nothing local running - the
/// shell the boot `LoadProfile` command fills with the local children.
pub(crate) fn boot_state(config: Config) -> AppState {
    app_state(config, None)
}

/// [`boot_state`] with config-file context and the active profile named,
/// as the runner assembles it from a persisted selection.
#[cfg(feature = "stt")]
pub(crate) fn boot_state_with_paths(config: Config, paths: AdminPaths) -> AppState {
    app_state(config, Some(paths))
}

/// Arms the state's empty speech facade to publish `factory`'s
/// deterministic workers on its one initial load, replacing the Whisper
/// backend for boot-path tests.
#[cfg(feature = "stt")]
pub(crate) fn arm_boot_speech(
    state: &mut AppState,
    factory: impl gateway_stt::test_fixtures::ModelFactory,
) {
    let policy = gateway_stt::test_fixtures::EnginePolicy::new(15, 500, false)
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
impl gateway_stt::test_fixtures::ModelFactory for GatedConstructionFactory {
    fn create(
        &self,
        mode: gateway_stt::test_fixtures::DecodeMode,
    ) -> Result<
        Option<Box<dyn gateway_stt::test_fixtures::Decoder>>,
        gateway_stt::test_fixtures::TranscribeError,
    > {
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

/// Polls `condition` with a bounded wait, for observing externally
/// visible state transitions.
pub(crate) async fn wait_until(what: &str, condition: impl Fn() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// An executor that parks every command until its token fires, then
/// settles it as cancelled - the shape of a provisioning download.
/// Commands without a token (unloads) settle immediately.
pub(crate) fn parking_executor() -> Arc<crate::commands::Executor> {
    use futures_util::future::BoxFuture;

    use crate::commands::Outcome;

    Arc::new(|_state, command, activity| {
        Box::pin(async move {
            // The activity lives as long as the parked body, as a real
            // command's would, so the hub reads busy while it waits.
            let _activity = activity;
            let label = command.label();
            let Some(token) = command.token() else {
                return Ok(label);
            };
            token.cancelled().await;
            Err(GatewayError::CommandCancelled(label))
        }) as BoxFuture<'static, Outcome>
    })
}

/// A fake OpenAI backend on an ephemeral loopback port answering every
/// chat completion with a canned reply, so a routed request completes
/// end to end.
#[cfg(any(feature = "local", feature = "stt"))]
pub(crate) async fn fake_chat_backend() -> SocketAddr {
    async fn completions(
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "id": "cmpl-test",
            "object": "chat.completion",
            "model": body["model"].as_str().unwrap_or(""),
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "pong" },
                "finish_reason": "stop"
            }]
        }))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the backend listener binds");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(
            listener,
            axum::Router::new().route("/chat/completions", axum::routing::post(completions)),
        )
        .await;
    });
    addr
}

/// A state whose key is `test-token` over a config carrying an empty
/// `[workshop]` section, as the speech-route auth tests serve.
pub(crate) fn workshop_state() -> AppState {
    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [workshop]\n",
    )
    .expect("config parses");
    app_state(config, None)
}

/// A state whose key is `test-token`, with `trust_loopback` set as
/// given (absent means the default).
pub(crate) fn loopback_state(trust_loopback: Option<bool>) -> AppState {
    let trust = trust_loopback.map_or(String::new(), |trust| format!("trust_loopback = {trust}\n"));
    let config = Config::from_toml_str(&format!(
        "config-version = 0\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n{trust}"
    ))
    .expect("config parses");
    app_state(config, None)
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
            "config-version = 0\n\
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
            "config-version = 0\n\
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
            "config-version = 0\n\
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
