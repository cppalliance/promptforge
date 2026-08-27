//! Voice model provisioning: fetching the configured whisper models through
//! the gateway's cache API once the gateway is reachable, then activating
//! the voice engine from the cached paths.
//!
//! The task spawned by [`spawn`] subscribes to the heartbeat's reachability
//! flag and, whenever the gateway answers and the voice engine is not
//! loaded, calls `POST /v1/cache` for each configured model source. A cache
//! hit answers immediately with the cached path; a miss streams download
//! progress events (forwarded to the status bar) that end in a terminal
//! `ready` event carrying the path. When both models resolve, the engine
//! loads from the resolved paths - on the blocking pool, since model
//! loading waits on worker-thread init - and the shared [`VoiceSlot`] is
//! activated, so the next `/voice` session transcribes. One successful
//! provisioning ends the task; a failure is logged and reported on the
//! status bus, and the next gateway reconnect retries - a retry hits the
//! cache for every blob the failed attempt already fetched, so it is cheap.
//!
//! The task stops through its [`Provision`] handle: the stop signal wins
//! the loop's selects, so shutdown never waits out a watch change or an
//! in-flight cache call. The server runs the shutdown inside its
//! graceful-shutdown future, next to the heartbeat's.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use tokio::sync::oneshot;

use crate::config::VoiceConfig;
use crate::gateway::{CacheEvent, CacheResponse, GatewayClient, GatewayError};
use crate::heartbeat::GatewayHealth;
use crate::protocol::Activity;
use crate::push::Push;
use crate::transcribe::{TranscribeError, VoiceEngine, VoiceSlot};

/// A running provisioning task.
///
/// [`Provision::shutdown`] signals the task to stop and awaits it. Dropping
/// the handle without shutting down still stops the task at its next
/// select point, because the closed channel resolves the stop branch.
#[derive(Debug)]
pub(crate) struct Provision {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Provision {
    /// Signals the task to stop and waits for it to finish.
    pub(crate) async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Spawns the provisioning task against `client`, reporting through
/// `push`, waiting on `health`, and activating `voice` on success.
pub(crate) fn spawn(
    client: GatewayClient,
    push: Push,
    health: GatewayHealth,
    voice: VoiceSlot,
    config: VoiceConfig,
) -> Provision {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(&client, &push, &health, &voice, &config, &mut stopped).await;
    });
    Provision {
        stop: Some(stop),
        task: Some(task),
    }
}

/// The task loop: wait for a reachable gateway, provision, and either
/// finish (success) or park until the next reachability change (failure),
/// so a persistent failure can never spin.
async fn run(
    client: &GatewayClient,
    push: &Push,
    health: &GatewayHealth,
    voice: &VoiceSlot,
    config: &VoiceConfig,
    stop: &mut oneshot::Receiver<()>,
) {
    // A loaded engine is never re-provisioned; a configuration with no
    // resolvable interim model can never succeed.
    if voice.is_active() || !can_provision(config) {
        return;
    }
    let mut reachable = health.subscribe();
    loop {
        while !*reachable.borrow_and_update() {
            tokio::select! {
                _ = &mut *stop => return,
                changed = reachable.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
        if voice.is_active() {
            return;
        }
        let outcome = tokio::select! {
            _ = &mut *stop => return,
            outcome = provision_once(client, push, voice, config) => outcome,
        };
        match outcome {
            Ok(()) => return,
            Err(error) => {
                tracing::warn!(%error, "voice model provisioning failed");
                report_failure(push, &error);
            }
        }
        // A failed attempt is retried on the next reconnect and only then:
        // wait for the flag to move before looping.
        tokio::select! {
            _ = &mut *stop => return,
            changed = reachable.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

/// Whether provisioning could ever resolve the interim model: a local file
/// to load, or a source URL to fetch.
fn can_provision(config: &VoiceConfig) -> bool {
    config.interim_model.is_file() || !config.interim_source.is_empty()
}

/// Resolves both whisper models to local paths - the interim model, and
/// the final-pass model when one is configured or sourced - then loads the
/// voice engine from them and activates the slot.
async fn provision_once(
    client: &GatewayClient,
    push: &Push,
    voice: &VoiceSlot,
    config: &VoiceConfig,
) -> Result<(), ProvisionError> {
    let interim =
        resolve_model(client, push, &config.interim_model, &config.interim_source).await?;
    let final_pass = resolve_final(client, push, config).await?;
    let mut resolved = config.clone();
    resolved.interim_model = interim;
    resolved.final_model = final_pass.unwrap_or_default();
    // VoiceEngine::new blocks on the worker threads' model init, so it
    // runs on the blocking pool and never stalls the executor.
    let engine = tokio::task::spawn_blocking(move || VoiceEngine::new(&resolved))
        .await
        .map_err(ProvisionError::EngineTask)?
        .map_err(ProvisionError::LoadEngine)?;
    voice.activate(engine);
    push.push_status_update(
        "Voice ready",
        "the whisper models are loaded; push-to-talk transcription is available",
        Activity::General,
    );
    Ok(())
}

/// Resolves one model to a local path: the configured path when the file
/// exists, otherwise a cache fetch of its source URL.
async fn resolve_model(
    client: &GatewayClient,
    push: &Push,
    path: &Path,
    source: &str,
) -> Result<PathBuf, ProvisionError> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if source.is_empty() {
        return Err(ProvisionError::NoSource {
            path: path.to_path_buf(),
        });
    }
    cache_fetch(client, push, source).await
}

/// Resolves the optional final-pass model. A configured final path that is
/// missing with no source URL degrades to no final pass rather than
/// failing provisioning: the final pass is an enhancement, and takes then
/// close with the interim model as they do when no final model is set.
async fn resolve_final(
    client: &GatewayClient,
    push: &Push,
    config: &VoiceConfig,
) -> Result<Option<PathBuf>, ProvisionError> {
    if config.final_model.is_file() {
        return Ok(Some(config.final_model.clone()));
    }
    if config.final_source.is_empty() {
        if !config.final_model.as_os_str().is_empty() {
            push.push_status_update(
                "Voice final pass unavailable",
                format!(
                    "{} is missing and no final_source is configured; takes close with the interim model",
                    config.final_model.display()
                ),
                Activity::General,
            );
        }
        return Ok(None);
    }
    cache_fetch(client, push, &config.final_source)
        .await
        .map(Some)
}

/// The label filename for a source URL: its last path segment, or the
/// whole source when it has none.
fn source_filename(source: &str) -> &str {
    source.rsplit('/').next().unwrap_or(source)
}

/// Ensures the blob at `source` is cached, returning its local path. A
/// cache hit answers immediately; a miss consumes the download event
/// stream, forwarding each progress sample to the status bar, until the
/// terminal `ready` or `error` event.
///
/// The stream carries no stall timeout, matching the chat stream's
/// posture: a download that stops mid-way holds the attempt until the
/// connection errors or the server shuts down (the stop signal still wins
/// the task's select, so shutdown stays prompt, and startup never waits
/// on provisioning). A reconnect does not interrupt a stalled attempt.
async fn cache_fetch(
    client: &GatewayClient,
    push: &Push,
    source: &str,
) -> Result<PathBuf, ProvisionError> {
    match client.cache_ensure(source).await? {
        CacheResponse::Buffered(answer) if answer.status.is_success() => {
            match serde_json::from_slice::<CacheEvent>(&answer.body) {
                Ok(CacheEvent::Ready { path }) => Ok(path),
                Ok(_) => Err(ProvisionError::Malformed(
                    "a cache hit answered an event other than ready".to_string(),
                )),
                Err(error) => Err(ProvisionError::Malformed(format!(
                    "the cache hit answer is not a cache event: {error}"
                ))),
            }
        }
        CacheResponse::Buffered(answer) => Err(ProvisionError::Declined(answer.status)),
        CacheResponse::Download { mut payloads, .. } => loop {
            let filename = source_filename(source);
            let Some(item) = payloads.next().await else {
                return Err(ProvisionError::Malformed(
                    "the download stream ended without a terminal event".to_string(),
                ));
            };
            let payload = item?;
            let event = serde_json::from_str::<CacheEvent>(&payload).map_err(|error| {
                ProvisionError::Malformed(format!("a download event is not valid JSON: {error}"))
            })?;
            match event {
                CacheEvent::Downloading { bytes, total } => {
                    // A null total means the upstream sent no
                    // Content-Length; it crosses the wire as a 0 total,
                    // which the status bar clamps to a degenerate bar.
                    push.push_progress(
                        format!("Downloading {filename}"),
                        format!("{source} through the gateway cache"),
                        bytes,
                        total.unwrap_or(0),
                        Activity::General,
                    );
                }
                CacheEvent::Ready { path } => {
                    push.push_status_update(
                        "Download complete",
                        format!("{filename} is cached at {}", path.display()),
                        Activity::General,
                    );
                    return Ok(path);
                }
                CacheEvent::Error { message } => return Err(ProvisionError::Download(message)),
            }
        },
    }
}

/// Reports a provisioning failure. A transport failure means the gateway
/// is not there - the heartbeat's story to tell - so it speaks at Info
/// with the retry note; every other failure is a user-visible error.
fn report_failure(push: &Push, error: &ProvisionError) {
    match error {
        ProvisionError::Transport(_) => push.push_status_update(
            "Voice models wait on the gateway",
            format!("{error}; provisioning retries when the gateway reconnects"),
            Activity::General,
        ),
        _ => push.push_failure(
            "Voice provisioning failed",
            format!("{error}; voice stays disabled; a gateway reconnect retries"),
            Activity::General,
        ),
    }
}

/// A provisioning failure. Voice stays disabled and the app runs on; the
/// task retries on the next gateway reconnect.
#[derive(Debug, thiserror::Error)]
enum ProvisionError {
    /// The cache request or its event stream failed at the transport level.
    #[error("gateway cache transport error")]
    Transport(#[source] GatewayError),

    /// The gateway answered the cache request with a non-success status.
    #[error("gateway declined the cache request with {0}")]
    Declined(reqwest::StatusCode),

    /// The gateway reported a download failure on the event stream.
    #[error("model download failed: {0}")]
    Download(String),

    /// The cache answer did not match the API's event shapes.
    #[error("unexpected cache response: {0}")]
    Malformed(String),

    /// A model file is missing and no source URL is configured for it.
    #[error("{} is missing and no source URL is configured", path.display())]
    NoSource {
        /// The configured model path that does not exist.
        path: PathBuf,
    },

    /// The blocking task building the voice engine itself failed.
    #[error("voice engine load task failed")]
    EngineTask(#[source] tokio::task::JoinError),

    /// The resolved whisper models could not be loaded.
    #[error("load voice engine")]
    LoadEngine(#[source] TranscribeError),
}

impl From<GatewayError> for ProvisionError {
    fn from(source: GatewayError) -> Self {
        Self::Transport(source)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use tokio::sync::broadcast;

    use crate::catalog::CatalogBus;
    use crate::protocol::{Severity, StatusBarUpdate};
    use crate::status::StatusBus;
    use crate::transcribe::fixtures;

    const INTERIM_SOURCE: &str = "http://gateway.test/models/ggml-large-v3-turbo.bin";
    const FINAL_SOURCE: &str = "http://gateway.test/models/ggml-large-v3.bin";

    /// A voice config with both sources set and no local model paths: the
    /// generated-template first-run shape.
    fn sourced_config() -> VoiceConfig {
        VoiceConfig {
            interim_source: INTERIM_SOURCE.to_string(),
            final_source: FINAL_SOURCE.to_string(),
            ..VoiceConfig::default()
        }
    }

    /// A mock `POST /v1/cache` answering every source with an immediate
    /// ready event pointing at the whisper test fixture, so the activated
    /// engine is real.
    async fn mock_cache_ready(axum::Json(body): axum::Json<serde_json::Value>) -> Response {
        assert!(body["source"].is_string(), "the request names a source");
        axum::Json(serde_json::json!({
            "path": fixtures::require_model(),
            "status": "ready",
        }))
        .into_response()
    }

    /// A mock cache whose answer flips under test control: 500 while
    /// `ready` is unset, an immediate ready event once set.
    async fn mock_cache_flippable(
        State(ready): State<Arc<AtomicBool>>,
        axum::Json(body): axum::Json<serde_json::Value>,
    ) -> Response {
        if !ready.load(Ordering::Relaxed) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({
                    "error": {"message": "cache broken", "code": "cache_error"}
                })),
            )
                .into_response();
        }
        assert!(body["source"].is_string(), "the request names a source");
        axum::Json(serde_json::json!({
            "path": fixtures::require_model(),
            "status": "ready",
        }))
        .into_response()
    }

    /// A mock cache answering with an SSE download stream whose terminal
    /// ready event points at the whisper test fixture. `events` is the
    /// progress prefix, verbatim.
    fn mock_cache_stream(events: &'static str) -> Router {
        let model = fixtures::require_model();
        Router::new().route(
            "/v1/cache",
            post(move || {
                let model = model.clone();
                async move {
                    let ready = serde_json::json!({"status": "ready", "path": model}).to_string();
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                        format!("{events}data: {ready}\n\n"),
                    )
                }
            }),
        )
    }

    /// Binds `app` on a free loopback port and returns its base URL.
    async fn serve(app: Router) -> String {
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

    /// Receives the next status update within a generous deadline.
    async fn next_update(rx: &mut broadcast::Receiver<StatusBarUpdate>) -> StatusBarUpdate {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a status update arrives within the deadline")
            .expect("the status bus is open")
    }

    /// Polls the slot every 10 ms until the engine activates or the
    /// deadline passes. The deadline is generous: activation loads the
    /// fixture model onto real worker threads.
    async fn wait_active(slot: &VoiceSlot) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            if slot.is_active() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        slot.is_active()
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn a_cache_hit_activates_the_engine_from_the_cached_paths() {
        let base_url = serve(Router::new().route("/v1/cache", post(mock_cache_ready))).await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let slot = VoiceSlot::default();
        // The health flag starts optimistic, so the task attempts
        // provisioning immediately, before any publish.
        let provision = spawn(
            client,
            Push::new(status, CatalogBus::new()),
            GatewayHealth::new(),
            slot.clone(),
            sourced_config(),
        );

        assert!(
            wait_active(&slot).await,
            "the engine activates within the deadline"
        );
        let engine = slot.engine().expect("the slot holds the engine");
        let text = engine
            .transcribe(fixtures::jfk_samples())
            .await
            .expect("transcription succeeds");
        assert!(
            text.to_lowercase().contains("country"),
            "the provisioned engine transcribes the fixture: {text:?}"
        );
        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Voice ready");
        assert_eq!(update.severity, Severity::Info);
        assert_eq!(update.activity, Activity::General);
        provision.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn a_failed_attempt_reports_an_error_and_retries_on_reconnect() {
        let ready = Arc::new(AtomicBool::new(false));
        let base_url = serve(
            Router::new()
                .route("/v1/cache", post(mock_cache_flippable))
                .with_state(Arc::clone(&ready)),
        )
        .await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let slot = VoiceSlot::default();
        let health = GatewayHealth::new();
        let provision = spawn(
            client,
            Push::new(status, CatalogBus::new()),
            health.clone(),
            slot.clone(),
            sourced_config(),
        );

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Voice provisioning failed");
        assert_eq!(update.severity, Severity::Error);
        assert!(!slot.is_active(), "a failed attempt activates nothing");

        // The gateway recovers: the reconnect drives a retry, which now
        // succeeds.
        ready.store(true, Ordering::Relaxed);
        health.publish(false);
        health.publish(true);
        assert!(
            wait_active(&slot).await,
            "the retry activates the engine within the deadline"
        );
        provision.shutdown().await;
    }

    #[tokio::test]
    async fn a_transport_failure_speaks_at_info_and_stays_inactive() {
        // Nothing listens on port 1, so the cache request fails to connect.
        let client = GatewayClient::new("http://127.0.0.1:1", "").expect("client builds in tests");
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let slot = VoiceSlot::default();
        let provision = spawn(
            client,
            Push::new(status, CatalogBus::new()),
            GatewayHealth::new(),
            slot.clone(),
            sourced_config(),
        );

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Voice models wait on the gateway");
        assert_eq!(update.severity, Severity::Info);
        assert!(!slot.is_active());
        provision.shutdown().await;
    }

    #[tokio::test]
    async fn a_config_without_sources_or_models_exits_immediately() {
        let client = GatewayClient::new("http://127.0.0.1:1", "").expect("client builds in tests");
        let provision = spawn(
            client,
            Push::new(StatusBus::new(), CatalogBus::new()),
            GatewayHealth::new(),
            VoiceSlot::default(),
            VoiceConfig::default(),
        );
        tokio::time::timeout(Duration::from_secs(5), provision.shutdown())
            .await
            .expect("the task exits without waiting on the gateway");
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn an_active_engine_is_not_reprovisioned() {
        let calls = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&calls);
        let base_url = serve(Router::new().route(
            "/v1/cache",
            post(move || {
                let seen = Arc::clone(&seen);
                async move {
                    seen.store(true, Ordering::Relaxed);
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }),
        ))
        .await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let slot = VoiceSlot::default();
        slot.activate(
            VoiceEngine::new(&VoiceConfig {
                interim_model: fixtures::require_model(),
                ..VoiceConfig::default()
            })
            .expect("the fixture model loads"),
        );
        let provision = spawn(
            client,
            Push::new(StatusBus::new(), CatalogBus::new()),
            GatewayHealth::new(),
            slot,
            sourced_config(),
        );
        provision.shutdown().await;
        assert!(
            !calls.load(Ordering::Relaxed),
            "a loaded engine is never re-provisioned"
        );
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn a_download_stream_activates_the_engine_at_ready() {
        let base_url = serve(mock_cache_stream(
            "data: {\"status\":\"downloading\",\"bytes\":5,\"total\":12}\n\n",
        ))
        .await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let slot = VoiceSlot::default();
        let mut config = sourced_config();
        config.final_source = String::new();
        let provision = spawn(
            client,
            Push::new(StatusBus::new(), CatalogBus::new()),
            GatewayHealth::new(),
            slot.clone(),
            config,
        );

        assert!(
            wait_active(&slot).await,
            "the stream's terminal ready activates the engine"
        );
        provision.shutdown().await;
    }

    #[tokio::test]
    async fn a_terminal_error_event_fails_the_attempt() {
        let base_url = serve(Router::new().route(
            "/v1/cache",
            post(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    "data: {\"status\":\"error\",\"message\":\"disk full\"}\n\n",
                )
            }),
        ))
        .await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let slot = VoiceSlot::default();
        let provision = spawn(
            client,
            Push::new(status, CatalogBus::new()),
            GatewayHealth::new(),
            slot.clone(),
            sourced_config(),
        );

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Voice provisioning failed");
        assert!(
            update.description.contains("disk full"),
            "the gateway's message reaches the status bar: {update:?}"
        );
        assert!(!slot.is_active());
        provision.shutdown().await;
    }

    /// A `/ws` client socket connected to a live workshop test server,
    /// plus the state pieces the provision task spawns with.
    struct Workshop {
        socket: tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        gateway: GatewayClient,
        push: Push,
        health: GatewayHealth,
        slot: VoiceSlot,
        voice: VoiceConfig,
        _tape_dir: tempfile::TempDir,
    }

    /// Builds a workshop router with the given voice config and gateway
    /// URL, binds it on a free loopback port, and connects a `/ws` client.
    async fn connect_workshop(gateway_url: String, voice: VoiceConfig) -> Workshop {
        let tape_dir = tempfile::TempDir::new().expect("tempdir");
        let config = crate::config::Config {
            gateway: crate::config::GatewayConfig {
                base_url: gateway_url,
                api_key: String::new(),
            },
            tape: crate::config::TapeConfig {
                path: tape_dir.path().join("tape.jsonl"),
            },
            server: crate::config::ServerConfig::default(),
            voice,
        };
        let state = crate::AppState::new(&config).expect("state builds in tests");
        let gateway = state.gateway_client().clone();
        let push = state.push();
        let health = state.health().clone();
        let slot = state.voice_slot();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the workshop test server");
        let addr = listener.local_addr().expect("workshop test server address");
        tokio::spawn(async move {
            axum::serve(listener, crate::router(state))
                .await
                .expect("workshop test server serves");
        });
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("connect to /ws");
        Workshop {
            socket,
            gateway,
            push,
            health,
            slot,
            voice: config.voice.clone(),
            _tape_dir: tape_dir,
        }
    }

    /// Reads one text frame off a `/ws` client socket and parses it as
    /// JSON.
    async fn read_frame(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> serde_json::Value {
        let message = socket
            .next()
            .await
            .expect("a frame follows")
            .expect("the frame is not a socket error");
        let text = message.into_text().expect("the frame is text");
        serde_json::from_str(&text).expect("the frame is JSON")
    }

    #[tokio::test]
    #[ignore = "requires whisper test fixtures (tests/fixtures/)"]
    async fn download_progress_flows_to_the_main_ws_status_feed() {
        // A mock cache answering with an SSE download stream: the first
        // sample has a null total (no Content-Length upstream), the second
        // a known total, then the terminal ready.
        let base_url = serve(mock_cache_stream(concat!(
            "data: {\"status\":\"downloading\",\"bytes\":5,\"total\":null}\n\n",
            "data: {\"status\":\"downloading\",\"bytes\":12,\"total\":12}\n\n",
        )))
        .await;
        let voice = VoiceConfig {
            interim_source: INTERIM_SOURCE.to_string(),
            ..VoiceConfig::default()
        };
        let mut workshop = connect_workshop(base_url, voice).await;

        // Park the task on an unreachable flag until the session's status
        // forwarder is subscribed, then flip reachable to fire the attempt.
        workshop.health.publish(false);
        let provision = spawn(
            workshop.gateway.clone(),
            workshop.push.clone(),
            workshop.health.clone(),
            workshop.slot.clone(),
            workshop.voice.clone(),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        workshop.health.publish(true);

        // Collect status frames until the terminal success frame.
        let mut frames = Vec::new();
        let collect = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let frame = read_frame(&mut workshop.socket).await;
                if frame["type"] != "status" {
                    continue;
                }
                let terminal = frame["label"] == "Voice ready";
                frames.push(frame);
                if terminal {
                    break;
                }
            }
        });
        collect
            .await
            .expect("the download sequence completes within the deadline");

        let labels: Vec<&str> = frames
            .iter()
            .map(|frame| frame["label"].as_str().expect("label is a string"))
            .collect();
        assert_eq!(
            labels,
            [
                "Downloading ggml-large-v3-turbo.bin",
                "Downloading ggml-large-v3-turbo.bin",
                "Download complete",
                "Voice ready",
            ],
            "progress samples, then the terminal pair: {labels:?}"
        );
        assert_eq!(
            frames[0]["progress"],
            serde_json::json!({"current": 5, "total": 0}),
            "a null total crosses the wire as 0"
        );
        assert_eq!(
            frames[1]["progress"],
            serde_json::json!({"current": 12, "total": 12})
        );
        assert_eq!(frames[0]["severity"], "info");
        assert_eq!(frames[0]["activity"], "general");
        assert!(
            frames[2]["progress"].is_null(),
            "the terminal download frame clears the progress bar"
        );
        provision.shutdown().await;
    }
}
