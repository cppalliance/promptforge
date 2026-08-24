//! Voice model provisioning: fetching the configured whisper models through
//! the gateway's cache API once the gateway is reachable.
//!
//! The task spawned by [`spawn`] subscribes to the heartbeat's reachability
//! flag and, whenever the gateway answers and the voice models are not
//! loaded, calls `POST /v1/cache` for each configured model source. A cache
//! hit answers immediately with the cached path; a miss streams download
//! progress events that end in a terminal `ready` event carrying the path.
//! The resolved paths are parked in the shared store for the voice engine's
//! deferred activation. One successful provisioning ends the task; a
//! failure is logged and reported on the status bus, and the next gateway
//! reconnect retries - a retry hits the cache for every blob the failed
//! attempt already fetched, so it is cheap.
//!
//! The task stops through its [`Provision`] handle: the stop signal wins
//! the loop's selects, so shutdown never waits out a watch change or an
//! in-flight cache call. The server runs the shutdown inside its
//! graceful-shutdown future, next to the heartbeat's.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use futures_util::StreamExt;
use tokio::sync::oneshot;

use crate::config::VoiceConfig;
use crate::gateway::{CacheEvent, CacheResponse, GatewayClient, GatewayError};
use crate::heartbeat::GatewayHealth;
use crate::status::{Activity, StatusBus};

/// The whisper model paths the provisioning task resolved through the
/// gateway cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelPaths {
    /// Cached path of the interim (streaming) model.
    interim: PathBuf,
    /// Cached path of the final-pass model, when one resolved.
    final_pass: Option<PathBuf>,
}

/// Where the provisioning task parks the resolved model paths: filled
/// exactly once, on the first successful provisioning.
pub(crate) type ModelPathStore = Arc<Mutex<Option<ModelPaths>>>;

/// Locks the store, recovering from poisoning the way the tape does: a
/// panicking writer cannot wedge the readers.
fn lock_store(store: &ModelPathStore) -> std::sync::MutexGuard<'_, Option<ModelPaths>> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

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
/// `status`, waiting on `health`, and parking resolved paths in `store`.
pub(crate) fn spawn(
    client: GatewayClient,
    status: StatusBus,
    health: GatewayHealth,
    store: ModelPathStore,
    config: VoiceConfig,
) -> Provision {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(&client, &status, &health, &store, &config, &mut stopped).await;
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
    status: &StatusBus,
    health: &GatewayHealth,
    store: &ModelPathStore,
    config: &VoiceConfig,
    stop: &mut oneshot::Receiver<()>,
) {
    // An enabled voice configuration already loaded its models at startup;
    // a configuration with no resolvable interim model can never succeed.
    if config.enabled() || !can_provision(config) {
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
        let outcome = tokio::select! {
            _ = &mut *stop => return,
            outcome = provision_once(client, config) => outcome,
        };
        match outcome {
            Ok(paths) => {
                *lock_store(store) = Some(paths);
                status.info(
                    "Voice models cached",
                    "the whisper models are in the gateway cache",
                    Activity::Voice,
                );
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "voice model provisioning failed");
                report_failure(status, &error);
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

/// Resolves both whisper models to local paths: the interim model, and the
/// final-pass model when one is configured or sourced.
async fn provision_once(
    client: &GatewayClient,
    config: &VoiceConfig,
) -> Result<ModelPaths, ProvisionError> {
    let interim = resolve_model(client, &config.interim_model, &config.interim_source).await?;
    let final_pass = resolve_final(client, config).await?;
    Ok(ModelPaths {
        interim,
        final_pass,
    })
}

/// Resolves one model to a local path: the configured path when the file
/// exists, otherwise a cache fetch of its source URL.
async fn resolve_model(
    client: &GatewayClient,
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
    cache_fetch(client, source).await
}

/// Resolves the optional final-pass model. A configured final path that is
/// missing with no source URL degrades to no final pass rather than
/// failing provisioning: the final pass is an enhancement, and takes then
/// close with the interim model as they do when no final model is set.
async fn resolve_final(
    client: &GatewayClient,
    config: &VoiceConfig,
) -> Result<Option<PathBuf>, ProvisionError> {
    if config.final_model.is_file() {
        return Ok(Some(config.final_model.clone()));
    }
    if config.final_source.is_empty() {
        return Ok(None);
    }
    cache_fetch(client, &config.final_source).await.map(Some)
}

/// Ensures the blob at `source` is cached, returning its local path. A
/// cache hit answers immediately; a miss consumes the download event
/// stream to its terminal `ready` or `error` event.
async fn cache_fetch(client: &GatewayClient, source: &str) -> Result<PathBuf, ProvisionError> {
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
                // Progress reporting to the observer lands with the status
                // wiring commit.
                CacheEvent::Downloading { .. } => {}
                CacheEvent::Ready { path } => return Ok(path),
                CacheEvent::Error { message } => return Err(ProvisionError::Download(message)),
            }
        },
    }
}

/// Reports a provisioning failure. A transport failure means the gateway
/// is not there - the heartbeat's story to tell - so it speaks at Info
/// with the retry note; every other failure is a user-visible error.
fn report_failure(status: &StatusBus, error: &ProvisionError) {
    match error {
        ProvisionError::Transport(_) => status.info(
            "Voice models wait on the gateway",
            format!("{error}; provisioning retries when the gateway reconnects"),
            Activity::Voice,
        ),
        _ => status.error(
            "Voice provisioning failed",
            format!("{error}; voice stays disabled; a gateway reconnect retries"),
            Activity::Voice,
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
}

impl From<GatewayError> for ProvisionError {
    fn from(source: GatewayError) -> Self {
        Self::Transport(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use tokio::sync::broadcast;

    use crate::status::{Severity, StatusBarUpdate};

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
    /// ready event whose path is `/cache/<filename>`.
    async fn mock_cache_ready(axum::Json(body): axum::Json<serde_json::Value>) -> Response {
        let source = body["source"].as_str().expect("source is a string");
        let filename = source.rsplit('/').next().expect("a URL has a tail");
        axum::Json(serde_json::json!({
            "path": format!("/cache/{filename}"),
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
        let source = body["source"].as_str().expect("source is a string");
        let filename = source.rsplit('/').next().expect("a URL has a tail");
        axum::Json(serde_json::json!({
            "path": format!("/cache/{filename}"),
            "status": "ready",
        }))
        .into_response()
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

    /// Polls the store every 10 ms until it holds paths or the deadline
    /// passes.
    async fn stored_paths(store: &ModelPathStore) -> Option<ModelPaths> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let paths = lock_store(store).clone();
            if paths.is_some() || std::time::Instant::now() >= deadline {
                return paths;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_cache_hit_stores_the_resolved_paths() {
        let base_url = serve(Router::new().route("/v1/cache", post(mock_cache_ready))).await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let store: ModelPathStore = Arc::new(Mutex::new(None));
        // The health flag starts optimistic, so the task attempts
        // provisioning immediately, before any publish.
        let provision = spawn(
            client,
            status,
            GatewayHealth::new(),
            Arc::clone(&store),
            sourced_config(),
        );

        let paths = stored_paths(&store)
            .await
            .expect("provisioning resolves both models");
        assert_eq!(
            paths.interim,
            PathBuf::from("/cache/ggml-large-v3-turbo.bin")
        );
        assert_eq!(
            paths.final_pass,
            Some(PathBuf::from("/cache/ggml-large-v3.bin"))
        );
        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Voice models cached");
        assert_eq!(update.severity, Severity::Info);
        assert_eq!(update.activity, Activity::Voice);
        provision.shutdown().await;
    }

    #[tokio::test]
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
        let store: ModelPathStore = Arc::new(Mutex::new(None));
        let health = GatewayHealth::new();
        let provision = spawn(
            client,
            status,
            health.clone(),
            Arc::clone(&store),
            sourced_config(),
        );

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Voice provisioning failed");
        assert_eq!(update.severity, Severity::Error);
        assert!(
            lock_store(&store).is_none(),
            "a failed attempt stores nothing"
        );

        // The gateway recovers: the reconnect drives a retry, which now
        // succeeds.
        ready.store(true, Ordering::Relaxed);
        health.publish(false);
        health.publish(true);
        let paths = stored_paths(&store)
            .await
            .expect("the retry resolves both models");
        assert_eq!(
            paths.interim,
            PathBuf::from("/cache/ggml-large-v3-turbo.bin")
        );
        provision.shutdown().await;
    }

    #[tokio::test]
    async fn a_transport_failure_speaks_at_info_and_stores_nothing() {
        // Nothing listens on port 1, so the cache request fails to connect.
        let client = GatewayClient::new("http://127.0.0.1:1", "").expect("client builds in tests");
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let store: ModelPathStore = Arc::new(Mutex::new(None));
        let provision = spawn(
            client,
            status,
            GatewayHealth::new(),
            Arc::clone(&store),
            sourced_config(),
        );

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Voice models wait on the gateway");
        assert_eq!(update.severity, Severity::Info);
        assert!(lock_store(&store).is_none());
        provision.shutdown().await;
    }

    #[tokio::test]
    async fn a_config_without_sources_or_models_exits_immediately() {
        let client = GatewayClient::new("http://127.0.0.1:1", "").expect("client builds in tests");
        let provision = spawn(
            client,
            StatusBus::new(),
            GatewayHealth::new(),
            Arc::new(Mutex::new(None)),
            VoiceConfig::default(),
        );
        tokio::time::timeout(Duration::from_secs(5), provision.shutdown())
            .await
            .expect("the task exits without waiting on the gateway");
    }

    #[tokio::test]
    async fn an_enabled_voice_config_is_not_provisioned() {
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
        let config = VoiceConfig {
            interim_model: PathBuf::from("models/ggml-large-v3-turbo.bin"),
            interim_source: INTERIM_SOURCE.to_string(),
            ..VoiceConfig::default()
        };
        let provision = spawn(
            client,
            StatusBus::new(),
            GatewayHealth::new(),
            Arc::new(Mutex::new(None)),
            config,
        );
        provision.shutdown().await;
        assert!(
            !calls.load(Ordering::Relaxed),
            "an enabled configuration loaded its models at startup"
        );
    }

    #[tokio::test]
    async fn a_download_stream_is_consumed_to_its_terminal_ready() {
        let base_url = serve(Router::new().route(
            "/v1/cache",
            post(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    concat!(
                        "data: {\"status\":\"downloading\",\"bytes\":5,\"total\":12}\n\n",
                        "data: {\"status\":\"downloading\",\"bytes\":12,\"total\":12}\n\n",
                        "data: {\"status\":\"ready\",\"path\":\"/cache/ggml.bin\"}\n\n",
                    ),
                )
            }),
        ))
        .await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let store: ModelPathStore = Arc::new(Mutex::new(None));
        let mut config = sourced_config();
        config.final_source = String::new();
        let provision = spawn(
            client,
            StatusBus::new(),
            GatewayHealth::new(),
            Arc::clone(&store),
            config,
        );

        let paths = stored_paths(&store)
            .await
            .expect("the stream resolves the interim model");
        assert_eq!(paths.interim, PathBuf::from("/cache/ggml.bin"));
        assert_eq!(paths.final_pass, None, "no final source, no final pass");
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
        let store: ModelPathStore = Arc::new(Mutex::new(None));
        let provision = spawn(
            client,
            status,
            GatewayHealth::new(),
            Arc::clone(&store),
            sourced_config(),
        );

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Voice provisioning failed");
        assert!(
            update.description.contains("disk full"),
            "the gateway's message reaches the status bar: {update:?}"
        );
        assert!(lock_store(&store).is_none());
        provision.shutdown().await;
    }
}
