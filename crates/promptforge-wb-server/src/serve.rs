//! In-process serving: the workbench server on a dedicated thread.
//!
//! [`spawn`] builds the shared state, binds the listener, and serves on its
//! own thread with its own tokio runtime, so an embedding binary (the
//! desktop shell, or the server binary itself) keeps its main thread. The
//! call blocks until the listener is bound - that bind is the readiness
//! signal - and the returned [`ServerHandle`] carries the base URL and a
//! graceful-shutdown switch.

use std::sync::mpsc;
use std::thread::JoinHandle;

use crate::app::{AppError, AppState, router};
use crate::config::Config;
use crate::heartbeat;
use crate::provision;

/// A running workbench server on its own thread.
///
/// Dropping the handle without calling [`ServerHandle::shutdown`] still
/// signals the server to stop, but does not wait for it.
#[derive(Debug)]
pub struct ServerHandle {
    url: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<std::io::Result<()>>>,
}

impl ServerHandle {
    /// Returns the base URL the server is listening on, for example
    /// `http://127.0.0.1:7910`.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Signals graceful shutdown and waits for the server thread to finish.
    ///
    /// # Errors
    /// Returns `std::io::Error` if the server stopped with an error or the
    /// server thread panicked.
    pub fn shutdown(mut self) -> std::io::Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.join_inner()
    }

    /// Waits for the server thread to finish on its own, without signaling
    /// shutdown.
    ///
    /// # Errors
    /// Returns `std::io::Error` if the server stopped with an error or the
    /// server thread panicked.
    pub fn join(mut self) -> std::io::Result<()> {
        self.join_inner()
    }

    fn join_inner(&mut self) -> std::io::Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        match thread.join() {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::other("workbench server thread panicked")),
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

/// A failure to start the in-process workbench server.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpawnError {
    /// The shared state (gateway client, tape, voice engine) could not be
    /// built.
    #[non_exhaustive]
    #[error("build shared state")]
    State(#[source] AppError),

    /// An I/O failure: the listener bind failed, the bound address could
    /// not be read, or the server thread could not be spawned.
    #[non_exhaustive]
    #[error("start workbench server")]
    Io(#[source] std::io::Error),
}

/// Spawns the workbench server on a dedicated thread and blocks until the
/// listener is bound.
///
/// The bound listener is the readiness signal: when this returns `Ok`, the
/// server is accepting connections at [`ServerHandle::url`].
///
/// # Errors
/// Returns [`SpawnError::State`] if the shared state cannot be built (a bad
/// tape path or whisper model), and [`SpawnError::Io`] if the bind fails or
/// the server thread cannot be spawned.
pub fn spawn(config: Config) -> Result<ServerHandle, SpawnError> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let thread = std::thread::Builder::new()
        .name("promptforge-wb-server".to_string())
        .spawn(move || serve_thread(config, ready_tx, shutdown_rx))
        .map_err(SpawnError::Io)?;
    match ready_rx.recv() {
        Ok(Ok(url)) => Ok(ServerHandle {
            url,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err(SpawnError::Io(std::io::Error::other(
                "workbench server thread exited before binding",
            )))
        }
    }
}

/// The server thread's body: build a runtime, build state, bind, signal
/// readiness through `ready`, then serve until `shutdown` resolves.
///
/// Startup failures are reported through `ready`; only serving failures
/// become the thread's return value.
fn serve_thread(
    config: Config,
    ready: mpsc::Sender<Result<String, SpawnError>>,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> std::io::Result<()> {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(SpawnError::Io(error)));
            return Ok(());
        }
    };
    runtime.block_on(async move {
        let state = match AppState::new(&config) {
            Ok(state) => state,
            Err(error) => {
                let _ = ready.send(Err(SpawnError::State(error)));
                return Ok(());
            }
        };
        let listener = match tokio::net::TcpListener::bind(&config.server.bind).await {
            Ok(listener) => listener,
            Err(error) => {
                let _ = ready.send(Err(SpawnError::Io(error)));
                return Ok(());
            }
        };
        let address = listener.local_addr()?;
        let _ = ready.send(Ok(format!("http://{address}")));
        // The heartbeat and the voice provisioning task start with serving
        // and stop inside the same graceful-shutdown signal, so they never
        // outlive the server.
        let heartbeat = heartbeat::spawn(
            state.gateway_client().clone(),
            state.status(),
            state.health().clone(),
            state.catalog(),
            heartbeat::HEARTBEAT_INTERVAL,
        );
        let provision = provision::spawn(
            state.gateway_client().clone(),
            state.status(),
            state.health().clone(),
            state.voice_slot(),
            config.voice.clone(),
        );
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async move {
                let _ = shutdown.await;
                heartbeat.shutdown().await;
                provision.shutdown().await;
            })
            .await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    use crate::config::{GatewayConfig, ServerConfig, TapeConfig, VoiceConfig};

    fn test_config(bind: &str, tape_dir: &Path) -> Config {
        Config {
            gateway: GatewayConfig {
                base_url: "http://127.0.0.1:1".to_string(),
                api_key: "test-key".to_string(),
            },
            tape: TapeConfig {
                path: tape_dir.join("tape.jsonl"),
            },
            server: ServerConfig {
                bind: bind.to_string(),
                open_browser: false,
            },
            voice: VoiceConfig::default(),
        }
    }

    #[tokio::test]
    async fn readiness_means_the_health_endpoint_answers() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn(test_config("127.0.0.1:0", dir.path())).expect("server spawns");
        let url = server.url().to_string();
        assert!(
            url.starts_with("http://127.0.0.1:"),
            "the URL carries the bound loopback address: {url}"
        );

        let response = reqwest::get(format!("{url}/health"))
            .await
            .expect("the health endpoint answers once spawn returns");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = response.text().await.expect("the health body reads");
        assert_eq!(body, r#"{"status":"serving"}"#);

        server.shutdown().expect("graceful shutdown succeeds");
    }

    /// The test config points the gateway at port 1, which never listens:
    /// the server must still boot and serve - the UI and its own health
    /// endpoint do not depend on the gateway, and the heartbeat reports
    /// the outage instead of failing startup.
    #[tokio::test]
    async fn the_server_boots_and_serves_the_ui_with_an_unreachable_gateway() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn(test_config("127.0.0.1:0", dir.path())).expect("server spawns");
        let url = server.url().to_string();

        let health = reqwest::get(format!("{url}/health"))
            .await
            .expect("the health endpoint answers");
        assert_eq!(health.status(), reqwest::StatusCode::OK);
        let index = reqwest::get(format!("{url}/"))
            .await
            .expect("the UI answers");
        assert_eq!(index.status(), reqwest::StatusCode::OK);

        server.shutdown().expect("graceful shutdown succeeds");
    }

    /// A configured-but-missing voice model with no source URL degrades to
    /// disabled voice with a status-bar explanation; it must never fail
    /// startup.
    #[tokio::test]
    async fn a_missing_voice_model_without_a_source_still_boots() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut config = test_config("127.0.0.1:0", dir.path());
        config.voice.interim_model = std::path::PathBuf::from("definitely-missing-model.bin");
        let server = spawn(config).expect("server spawns with voice degraded");
        let url = server.url().to_string();

        let health = reqwest::get(format!("{url}/health"))
            .await
            .expect("the health endpoint answers");
        assert_eq!(health.status(), reqwest::StatusCode::OK);

        server.shutdown().expect("graceful shutdown succeeds");
    }

    #[tokio::test]
    async fn shutdown_releases_the_bound_port() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn(test_config("127.0.0.1:0", dir.path())).expect("server spawns");
        let address = server
            .url()
            .strip_prefix("http://")
            .expect("the URL is http")
            .to_string();
        server.shutdown().expect("graceful shutdown succeeds");

        let listener = tokio::net::TcpListener::bind(&address)
            .await
            .expect("the port is free after shutdown");
        drop(listener);
    }

    #[test]
    fn an_unopenable_tape_fails_spawn_with_state_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config = test_config("127.0.0.1:0", &dir.path().join("missing"));
        let error = spawn(config).expect_err("an unopenable tape must fail spawn");
        assert!(
            matches!(error, SpawnError::State(_)),
            "expected State, got {error:?}"
        );
    }

    #[test]
    fn a_bind_conflict_fails_spawn_with_io_error() {
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blocker");
        let address = blocker.local_addr().expect("blocker address");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config = test_config(&address.to_string(), dir.path());
        let error = spawn(config).expect_err("a taken port must fail spawn");
        assert!(
            matches!(error, SpawnError::Io(_)),
            "expected Io, got {error:?}"
        );
    }
}
