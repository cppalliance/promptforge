//! In-process serving: the workshop server on a dedicated thread.
//!
//! [`spawn`] builds the shared state, binds the listener, and serves on its
//! own thread with its own tokio runtime, so an embedding binary (the
//! desktop shell, or the server binary itself) keeps its main thread. The
//! call blocks until the listener is bound - that bind is the readiness
//! signal - and the returned [`ServerHandle`] carries the base URL and a
//! graceful-shutdown switch. The stop side is bounded: a watchdog gives
//! in-flight connections a grace window to drain and then tears the runtime
//! down anyway, and a stopped barrier reports [`Termination`] back through
//! [`ServerHandle::shutdown`], so a held socket can never park the host.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::app::{AppState, StateError, router, state_with_gateway};
use crate::config::Config;
use crate::gateway_progress;
use crate::heartbeat;
use crate::progress;
use crate::resolve::ResolvedGateway;

/// How long a signaled shutdown waits for in-flight connections to drain
/// before the watchdog abandons the graceful path. axum's drain waits on
/// every connection it still tracks - a request wedged mid-body never
/// drains - so this bound keeps one stuck client from parking the host's
/// join forever. (Held WebSockets detach from the drain at upgrade.)
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How long either ending waits for the runtime to tear down. Cancelled
/// async tasks collapse at their next yield, but a wedged blocking task
/// would otherwise hold the runtime's drop - and with it the host's join -
/// open indefinitely.
const RUNTIME_TEARDOWN: Duration = Duration::from_secs(1);

type RouteFactory = Box<dyn FnOnce(&AppState) -> axum::Router + Send>;

/// How a [`ServerHandle::shutdown`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Termination {
    /// Every in-flight connection drained within the grace window.
    Graceful,
    /// The grace window lapsed with connections still held open, and the
    /// runtime was torn down anyway.
    Forced,
}

/// A running workshop server on its own thread.
///
/// Dropping the handle without calling [`ServerHandle::shutdown`] still
/// signals the server to stop, but does not wait for it.
#[derive(Debug)]
pub struct ServerHandle {
    url: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    stopped: mpsc::Receiver<Termination>,
    thread: Option<JoinHandle<std::io::Result<()>>>,
}

impl ServerHandle {
    /// Returns the base URL the server is listening on, for example
    /// `http://127.0.0.1:7910`.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Signals shutdown and waits for the server thread to finish,
    /// reporting how the stop ended.
    ///
    /// The wait is bounded by the server's own watchdog: in-flight
    /// connections get a grace window to drain, and when one outlasts it
    /// (a request wedged mid-body), the runtime is torn down anyway and
    /// the stop reports [`Termination::Forced`].
    ///
    /// # Errors
    /// Returns `std::io::Error` if the server stopped with an error or the
    /// server thread panicked.
    pub fn shutdown(mut self) -> std::io::Result<Termination> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let stopped = self.stopped.recv();
        self.join_inner()?;
        // The barrier is sent on every post-readiness path, so a missing
        // report means the thread died without running its teardown.
        stopped.map_err(|_| std::io::Error::other("the server thread stopped without reporting"))
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
            Err(_) => Err(std::io::Error::other("workshop server thread panicked")),
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

/// A failure to start the in-process workshop server: rich, init-only, and
/// never sent over the wire, so `#[from]` conveniences are welcome here.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpawnError {
    /// The gateway endpoint could not be resolved, or the shared state
    /// could not be built.
    #[non_exhaustive]
    #[error("build shared state")]
    State(#[from] StateError),

    /// An I/O failure: the listener bind failed, the bound address could
    /// not be read, or the server thread could not be spawned.
    #[non_exhaustive]
    #[error("start workshop server")]
    Io(#[from] std::io::Error),
}

/// Spawns the workshop server on a dedicated thread and blocks until the
/// listener is bound.
///
/// The bound listener is the readiness signal: when this returns `Ok`, the
/// server is accepting connections at [`ServerHandle::url`].
///
/// # Errors
/// Returns [`SpawnError::State`] if the gateway endpoint cannot be
/// resolved (no live connection file and no explicit `[gateway]` config)
/// or the shared state cannot be built, and [`SpawnError::Io`] if the
/// bind fails or the server thread cannot be spawned.
pub fn spawn(config: Config) -> Result<ServerHandle, SpawnError> {
    spawn_inner(
        config,
        None,
        SHUTDOWN_GRACE,
        Box::new(|_| axum::Router::new()),
    )
}

/// Spawns the workshop server against an already-resolved gateway
/// endpoint, with gateway-owned routes merged into its loopback listener.
///
/// `gateway` skips connection-file discovery: a host attaching routes
/// holds its own endpoint - the merged gateway hosting the workshop
/// in-process, whose own just-written connection file is not serving yet
/// when the workshop spawns - and discovery must never condemn that file
/// or attach the workshop to a foreign gateway. `routes` runs after
/// shared state construction on the server thread. It receives the state
/// so an owning gateway subsystem can attach to the Workshop status bus
/// without moving that subsystem into this crate.
///
/// # Errors
/// Returns [`SpawnError::State`] if shared state cannot be built, or
/// [`SpawnError::Io`] if the listener or server thread cannot start.
pub fn spawn_with_routes(
    config: Config,
    gateway: ResolvedGateway,
    routes: impl FnOnce(&AppState) -> axum::Router + Send + 'static,
) -> Result<ServerHandle, SpawnError> {
    spawn_inner(config, Some(gateway), SHUTDOWN_GRACE, Box::new(routes))
}

/// [`spawn`] with the shutdown grace window injectable, so tests prove the
/// forced path in milliseconds instead of waiting out the real window.
/// Discovery is bypassed, so a test never consults the real run directory.
#[cfg(test)]
fn spawn_with_grace(config: Config, grace: Duration) -> Result<ServerHandle, SpawnError> {
    let gateway = ResolvedGateway::from_config(&config.gateway);
    spawn_inner(
        config,
        Some(gateway),
        grace,
        Box::new(|_| axum::Router::new()),
    )
}

fn spawn_inner(
    config: Config,
    gateway: Option<ResolvedGateway>,
    grace: Duration,
    routes: RouteFactory,
) -> Result<ServerHandle, SpawnError> {
    // Discovery runs before the server thread starts: a resolution
    // failure is the plain no-gateway error, never a bind-then-fail.
    let gateway = match gateway {
        Some(gateway) => gateway,
        None => crate::resolve::resolve(&config.gateway).map_err(StateError::Resolution)?,
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (stopped_tx, stopped_rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("workshop-server".to_string())
        .spawn(move || {
            serve_thread(
                config,
                gateway,
                routes,
                ready_tx,
                shutdown_rx,
                &stopped_tx,
                grace,
            )
        })?;
    match ready_rx.recv() {
        Ok(Ok(url)) => Ok(ServerHandle {
            url,
            shutdown: Some(shutdown_tx),
            stopped: stopped_rx,
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err(SpawnError::Io(std::io::Error::other(
                "workshop server thread exited before binding",
            )))
        }
    }
}

/// The server thread's body: build a runtime, build state, bind, signal
/// readiness through `ready`, then serve until `shutdown` resolves. When it
/// does, a watchdog bounds the graceful drain at `grace` before tearing the
/// runtime down anyway, and `stopped` reports which of the two endings ran.
///
/// Startup failures are reported through `ready`; only serving failures
/// become the thread's return value.
fn serve_thread(
    config: Config,
    gateway: ResolvedGateway,
    routes: RouteFactory,
    ready: mpsc::Sender<Result<String, SpawnError>>,
    shutdown: tokio::sync::oneshot::Receiver<()>,
    stopped: &mpsc::Sender<Termination>,
    grace: Duration,
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
    let (outcome, result) = runtime.block_on(async move {
        let gateway_base_url = gateway.base_url().to_string();
        let gateway_api_key = gateway.api_key().to_string();
        let state = match state_with_gateway(&config, &gateway) {
            Ok(state) => state,
            Err(error) => {
                let _ = ready.send(Err(SpawnError::State(error)));
                return (Termination::Graceful, Ok(()));
            }
        };
        let app = router(state.clone()).merge(routes(&state));
        let listener = match reuse_bind(&config.server.bind) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = ready.send(Err(SpawnError::Io(error)));
                return (Termination::Graceful, Ok(()));
            }
        };
        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => return (Termination::Graceful, Err(error)),
        };
        let _ = ready.send(Ok(format!("http://{address}")));
        // The heartbeat, gateway progress subscriber, and progress renderer
        // start with serving and stop inside the same graceful-shutdown
        // signal, so they never outlive the server.
        let heartbeat = heartbeat::spawn(
            state.gateway_client().clone(),
            state.push(),
            state.health().clone(),
            heartbeat::HEARTBEAT_INTERVAL,
            state.backoff().clone(),
        );
        let renderer = progress::spawn(std::sync::Arc::clone(state.progress()), state.push());
        let subscriber = gateway_progress::spawn(
            gateway_base_url,
            gateway_api_key,
            std::sync::Arc::clone(state.progress()),
            state.health().clone(),
        );
        let (draining_tx, draining_rx) = tokio::sync::oneshot::channel();
        let serve = async {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown.await;
                    // Arm the watchdog before draining the background
                    // tasks, so the grace window bounds the whole stop.
                    let _ = draining_tx.send(());
                    heartbeat.shutdown().await;
                    renderer.shutdown().await;
                    subscriber.shutdown().await;
                })
                .await
        };
        // axum's graceful shutdown waits for every connection it still
        // tracks - a request wedged mid-body parks it, while WebSockets
        // detach at upgrade (both pinned by the shutdown tests); once the
        // window lapses the watchdog wins the select and the graceful
        // wait is abandoned with the serve future.
        let watchdog = async {
            let _ = draining_rx.await;
            tokio::time::sleep(grace).await;
        };
        tokio::select! {
            result = serve => (Termination::Graceful, result),
            () = watchdog => (Termination::Forced, Ok(())),
        }
    });
    // The barrier reports before teardown: the host's join is then bounded
    // by RUNTIME_TEARDOWN, not by whatever the abandoned tasks still hold.
    let _ = stopped.send(outcome);
    // A graceful serve-return is not quiescence: sessions that detached at
    // upgrade can still be running, so both endings take the bounded
    // teardown - cancelled tasks collapse at their next yield, and a wedged
    // blocking task is abandoned rather than allowed to park the host.
    // Never `process::exit` here - this crate runs inside host binaries.
    runtime.shutdown_timeout(RUNTIME_TEARDOWN);
    result
}

/// Binds a TCP listener with `SO_REUSEADDR` so a restart doesn't fail on
/// TIME_WAIT sockets from the previous instance.
fn reuse_bind(address: &str) -> std::io::Result<tokio::net::TcpListener> {
    let addr: std::net::SocketAddr = address
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(addr),
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    tokio::net::TcpListener::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    use crate::config::{AgentsConfig, GatewayConfig, ServerConfig};

    fn test_config(bind: &str, state_dir: &Path) -> Config {
        Config {
            gateway: GatewayConfig {
                base_url: "http://127.0.0.1:1".to_string(),
                api_key: "test-key".to_string(),
            },
            server: ServerConfig {
                bind: bind.to_string(),
                open_browser: false,
                state_dir: state_dir.to_path_buf(),
            },
            agents: AgentsConfig::default(),
        }
    }

    #[tokio::test]
    async fn readiness_means_the_health_endpoint_answers() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
            .expect("server spawns");
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
        let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
            .expect("server spawns");
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

    /// A grace window short enough that the forced path proves itself in
    /// milliseconds instead of stalling the suite.
    const TEST_GRACE: Duration = Duration::from_millis(200);

    /// Connects a WebSocket to the workshop socket and returns it for the
    /// caller to hold open.
    async fn hold_ws_open(
        url: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let address = url.strip_prefix("http://").expect("the URL is http");
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .expect("the chat socket connects");
        socket
    }

    /// Opens a raw connection and wedges it mid-request: the head promises
    /// a body that never fully arrives, so the handler waits on the body,
    /// no response begins, and the connection holds axum's graceful drain
    /// open until torn down. The head must pass the cross-site guard - a
    /// loopback `Host` and a JSON content type - or the guard answers 403
    /// without ever polling the body and nothing wedges.
    async fn wedge_http_connection(url: &str) -> tokio::net::TcpStream {
        use tokio::io::AsyncWriteExt as _;

        let address = url.strip_prefix("http://").expect("the URL is http");
        let mut wedged = tokio::net::TcpStream::connect(address)
            .await
            .expect("the raw connection opens");
        wedged
            .write_all(
                b"POST /workspace/grant HTTP/1.1\r\nhost: 127.0.0.1\r\n\
                  content-type: application/json\r\ncontent-length: 64\r\n\r\n{",
            )
            .await
            .expect("the wedged request head sends");
        // Give the accept loop and the handler a beat to pick the request
        // up, so the connection is in-flight before shutdown begins.
        tokio::time::sleep(Duration::from_millis(50)).await;
        wedged
    }

    /// The regression this step exists to prevent: a client that never
    /// closes its WebSocket must not park shutdown forever. The upgrade
    /// detaches the session from axum's graceful drain, so today this stop
    /// is even graceful; the assertion pins only the bound, which the
    /// watchdog keeps true however axum's connection tracking evolves.
    #[tokio::test]
    async fn a_held_websocket_does_not_block_shutdown_past_the_grace_window() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), TEST_GRACE)
            .expect("server spawns");
        let _held = hold_ws_open(server.url()).await;

        let begun = std::time::Instant::now();
        server
            .shutdown()
            .expect("shutdown returns despite the held socket");
        assert!(
            begun.elapsed() < Duration::from_secs(3),
            "shutdown must return shortly after the grace window, took {:?}",
            begun.elapsed()
        );
    }

    /// A connection wedged mid-request does hold the graceful drain open,
    /// so the watchdog must abandon the wait at the window and report the
    /// stop as forced.
    #[tokio::test]
    async fn a_wedged_http_connection_is_forced_out_at_the_grace_window() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), TEST_GRACE)
            .expect("server spawns");
        let _wedged = wedge_http_connection(server.url()).await;

        let begun = std::time::Instant::now();
        let outcome = server
            .shutdown()
            .expect("shutdown returns despite the wedged connection");
        assert_eq!(
            outcome,
            Termination::Forced,
            "an in-flight request cannot drain; the watchdog must force the stop"
        );
        assert!(
            begun.elapsed() < Duration::from_secs(3),
            "shutdown must return shortly after the grace window, took {:?}",
            begun.elapsed()
        );
    }

    #[tokio::test]
    async fn an_idle_shutdown_completes_gracefully_without_spending_the_window() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
            .expect("server spawns");

        let begun = std::time::Instant::now();
        let outcome = server.shutdown().expect("graceful shutdown succeeds");
        assert_eq!(
            outcome,
            Termination::Graceful,
            "nothing held the drain open"
        );
        assert!(
            begun.elapsed() < SHUTDOWN_GRACE,
            "an idle server must stop before the watchdog matters, took {:?}",
            begun.elapsed()
        );
    }

    /// The stopped barrier: when `shutdown` returns, the server is really
    /// gone - nothing listens on its address - even when the stop was
    /// forced.
    #[tokio::test]
    async fn the_stopped_barrier_reports_after_serving_has_ended() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), TEST_GRACE)
            .expect("server spawns");
        let address = server
            .url()
            .strip_prefix("http://")
            .expect("the URL is http")
            .to_string();
        let _wedged = wedge_http_connection(server.url()).await;

        let outcome = server.shutdown().expect("shutdown returns");
        assert_eq!(
            outcome,
            Termination::Forced,
            "the wedged connection forces the stop"
        );
        let refused = tokio::net::TcpStream::connect(&address).await;
        assert!(
            refused.is_err(),
            "the stopped barrier resolves only after serving has ended, yet {address} accepted"
        );
    }

    #[tokio::test]
    async fn shutdown_releases_the_bound_port() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
            .expect("server spawns");
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
    fn a_bind_conflict_fails_spawn_with_io_error() {
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blocker");
        let address = blocker.local_addr().expect("blocker address");
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config = test_config(&address.to_string(), dir.path());
        let error =
            spawn_with_grace(config, SHUTDOWN_GRACE).expect_err("a taken port must fail spawn");
        assert!(
            matches!(error, SpawnError::Io(_)),
            "expected Io, got {error:?}"
        );
    }
}
