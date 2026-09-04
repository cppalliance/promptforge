//! Application entry points: [`spawn`], [`run`], and the assembled [`Gateway`].
//!
//! [`spawn`] is the embedding seam: it loads configuration, assembles the
//! serving shell, and serves on a dedicated thread with its own tokio
//! runtime, so an embedding binary keeps its main thread. The call blocks
//! until the listener is bound - that bind is the readiness signal - and the
//! returned [`GatewayHandle`] carries the bound URL and a graceful-shutdown
//! switch. Provisioning is not on this path: the boot `LoadProfile` command
//! runs on the gateway's command queue after the bind. [`run`] is the binary
//! path: a thin wrapper that spawns, installs the
//! Ctrl-C handler, and joins. `Gateway` is the in-process assembly seam used
//! by both and by integration tests, which bind their own listener and drive
//! [`Gateway::serve`] with a caller-owned shutdown signal.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use tokio::net::TcpListener;

use gateway_config::{Config, ProfileName, Secret};

use crate::api_error::{ServeError, StartupError};
#[cfg(feature = "local")]
use crate::local::LocalRuntime;
use crate::routing::Routing;
use crate::{AppState, build_router};

/// Options for running the gateway. Built by the binary from parsed args.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Path to the single global configuration file. `None` runs boot
    /// discovery (beside the executable, the working directory, the user
    /// profile's `.promptforge` directory) and, when nothing is found,
    /// first-run generation into the profile location.
    pub config_path: Option<PathBuf>,
    /// Optional command-line profile override.
    pub profile: Option<ProfileName>,
    /// Directory the `gateway.json` connection file is written to after a
    /// successful bind; `None` uses the default run directory under the
    /// user profile's `.promptforge` directory.
    pub run_dir: Option<PathBuf>,
    /// Opens the Settings handoff URL in the default browser once the
    /// listener is bound - the binary's `--browser`, used by the
    /// installer's first run. Embedders leave this `false`.
    pub browser: bool,
}

impl ServeOptions {
    /// Builds serve options from the config path and optional profile override.
    #[must_use]
    pub fn new(
        config_path: Option<PathBuf>,
        profile: impl Into<Option<ProfileName>>,
    ) -> ServeOptions {
        ServeOptions {
            config_path,
            profile: profile.into(),
            run_dir: None,
            browser: false,
        }
    }

    /// Sets the connection-file run directory, for tests and portable
    /// installs; the default is the user profile's `.promptforge/run`.
    #[must_use]
    pub fn with_run_dir(mut self, run_dir: PathBuf) -> ServeOptions {
        self.run_dir = Some(run_dir);
        self
    }

    /// Sets whether to open the Settings page in the browser once bound.
    #[must_use]
    pub fn with_browser(mut self, browser: bool) -> ServeOptions {
        self.browser = browser;
        self
    }
}

/// Optional admin configuration path plus the active profile name.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct ProfilesContext {
    /// Single configuration path, enabling pending writes and persistence.
    pub config_path: Option<PathBuf>,
    /// The active profile name, reported by `GET /admin/status`.
    pub active: Option<ProfileName>,
}

impl ProfilesContext {
    /// Builds a context from an optional config path and active name.
    #[must_use]
    pub fn new(config_path: Option<PathBuf>, active: Option<ProfileName>) -> ProfilesContext {
        ProfilesContext {
            config_path,
            active,
        }
    }
}

/// A fully assembled, owning gateway.
///
/// Holds the live routing table, the server key, the web-search capability, and
/// the local model runtime, so dropping a `Gateway` terminates every managed
/// `llama-server` child. The type is opaque; assemble it with
/// [`Gateway::from_config`].
#[derive(Debug)]
#[non_exhaustive]
pub struct Gateway {
    state: AppState,
}

impl Gateway {
    /// Assemble the serving shell instantly: an empty routing table, no
    /// local runtime, no provisioning. Models arrive when the command
    /// queue's boot `LoadProfile` hot-swaps them into the live table; until
    /// then an unloaded but configured model earns a 503 naming the active
    /// command.
    ///
    /// [`from_config`](Self::from_config) is the eager alternative for tests
    /// and embedders: it provisions before returning.
    ///
    /// # Examples
    /// ```
    /// use gateway::{Config, Gateway, ProfilesContext};
    ///
    /// let toml = r#"
    /// config-version = 2
    ///
    /// [server]
    /// bind = "127.0.0.1:0"
    /// api_key = "secret"
    ///
    /// [[endpoint]]
    /// id = "e"
    /// protocol = "openai"
    /// base_url = "http://127.0.0.1:9"
    /// api_key = ""
    ///
    /// [[model]]
    /// name = "m"
    /// description = "a model"
    /// context = 8192
    /// upstream = "u"
    /// endpoints = ["e"]
    /// "#;
    /// let config = Config::from_toml_str(toml).unwrap();
    /// let gateway = Gateway::new(&config, ProfilesContext::default());
    /// let _router = gateway.router();
    /// ```
    #[must_use]
    pub fn new(config: &Config, profiles: ProfilesContext) -> Gateway {
        Self::new_with_hub(
            config,
            profiles,
            Arc::new(shared_progress::ProgressHub::new()),
        )
    }

    /// [`new`](Self::new) over a caller-provided progress hub, so the
    /// serving lifecycle's renderer watches the boot command's progress.
    pub(crate) fn new_with_hub(
        config: &Config,
        profiles: ProfilesContext,
        hub: Arc<shared_progress::ProgressHub>,
    ) -> Gateway {
        let active = config
            .active_profile()
            .map(|profile| profile.name().to_owned())
            .or_else(|| profiles.active.map(|name| name.to_string()));
        let model_allowlist = config
            .active_profile()
            .map(|profile| profile.models().to_vec());
        let state = AppState::from_parts(
            Arc::new(Routing::empty()),
            config.server_key(),
            Arc::new(config.clone()),
            #[cfg(feature = "local")]
            LocalRuntime::empty(),
            #[cfg(feature = "stt")]
            gateway_stt::SttRuntime::empty(gateway_stt::SttState::default()),
            #[cfg(feature = "web-search")]
            config.web_search_config(),
            profiles.config_path,
            crate::ProfileSelection {
                name: active,
                model_allowlist,
            },
            hub,
        );
        Gateway { state }
    }

    /// Assemble from a validated config. Provisions and starts local models.
    ///
    /// Profile switches derive subsets from this config's loaded catalog.
    ///
    /// # Errors
    /// Returns [`StartupError`] when local provisioning or routing construction
    /// fails.
    ///
    /// # Examples
    /// ```
    /// use gateway::{Config, Gateway, ProfilesContext};
    ///
    /// let toml = r#"
    /// config-version = 2
    ///
    /// [server]
    /// bind = "127.0.0.1:0"
    /// api_key = "secret"
    ///
    /// [[endpoint]]
    /// id = "e"
    /// protocol = "openai"
    /// base_url = "http://127.0.0.1:9"
    /// api_key = ""
    ///
    /// [[model]]
    /// name = "m"
    /// description = "a model"
    /// context = 8192
    /// upstream = "u"
    /// endpoints = ["e"]
    /// "#;
    /// let config = Config::from_toml_str(toml).unwrap();
    /// let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    /// let _router = gateway.router();
    /// ```
    pub fn from_config(
        config: &Config,
        profiles: ProfilesContext,
    ) -> Result<Gateway, StartupError> {
        Self::from_config_with_hub(
            config,
            profiles,
            Arc::new(shared_progress::ProgressHub::new()),
        )
    }

    /// [`from_config`](Self::from_config) over a caller-provided progress
    /// hub, so the serving lifecycle's renderer thread can watch startup
    /// provisioning.
    pub(crate) fn from_config_with_hub(
        config: &Config,
        profiles: ProfilesContext,
        hub: Arc<shared_progress::ProgressHub>,
    ) -> Result<Gateway, StartupError> {
        // Startup provisioning is the hub's first operation tree: it lives
        // for the provisioning call and detaches when the tree drops.
        #[cfg(feature = "local")]
        let local = {
            let tree = hub.operation();
            let progress = tree.register("startup", 1.0);
            let started =
                LocalRuntime::start(config, Some(&progress)).map_err(StartupError::provisioning);
            match &started {
                Ok(_) => progress.complete(),
                Err(_) => progress.fail(),
            }
            drop(tree);
            started?
        };
        // A headless build cannot honor a config declaring local models;
        // refuse at assembly rather than silently dropping them.
        #[cfg(not(feature = "local"))]
        if !config.local_models().is_empty() {
            return Err(StartupError::provisioning(std::io::Error::other(
                crate::LOCAL_MODELS_UNSUPPORTED,
            )));
        }
        #[cfg(feature = "stt")]
        let stt = {
            let tree = hub.operation();
            let progress = tree.register("startup-stt", 1.0);
            let state = gateway_stt::SttState::default();
            let started = gateway_stt::SttRuntime::start(config, state, Some(&progress))
                .map_err(StartupError::provisioning);
            match &started {
                Ok(_) => progress.complete(),
                Err(_) => progress.fail(),
            }
            drop(tree);
            started?
        };
        #[cfg(not(feature = "stt"))]
        if !config.stt_models().is_empty() {
            return Err(StartupError::provisioning(std::io::Error::other(
                crate::STT_RUNTIME_UNAVAILABLE,
            )));
        }
        let routing = Routing::from_config(config).map_err(StartupError::config)?;
        #[cfg(feature = "local")]
        let routing = routing
            .merge(local.models().iter().cloned())
            .map_err(StartupError::config)?;
        let active = config
            .active_profile()
            .map(|profile| profile.name().to_owned())
            .or_else(|| profiles.active.map(|name| name.to_string()));
        let model_allowlist = config
            .active_profile()
            .map(|profile| profile.models().to_vec());
        let state = AppState::from_parts(
            Arc::new(routing),
            config.server_key(),
            Arc::new(config.clone()),
            #[cfg(feature = "local")]
            local,
            #[cfg(feature = "stt")]
            stt,
            #[cfg(feature = "web-search")]
            config.web_search_config(),
            profiles.config_path,
            crate::ProfileSelection {
                name: active,
                model_allowlist,
            },
            hub,
        );
        Ok(Gateway { state })
    }

    /// The Axum router for this gateway.
    ///
    /// This is the crate's one deliberate, documented Axum integration point;
    /// the crate is an application, not a general library, so exposing an
    /// [`axum::Router`] here is intentional.
    ///
    /// The router carries no bound socket, so the host-authority wall is
    /// not installed; it exists on the [`serve`](Self::serve) path, where
    /// the bound address is known. Likewise `POST /shutdown` answers 202
    /// here without stopping anything: only `serve` selects on the
    /// route's signal.
    pub fn router(&self) -> axum::Router {
        build_router(self.state.clone(), None)
    }

    /// Bounded stdout/stderr tails captured from each running local
    /// `llama-server` child, keyed by configured model name.
    ///
    /// The per-attempt loopback credential is redacted from the captures.
    /// Embedding hosts use this to verify what a child actually reported -
    /// that a CUDA build staged its embedded bundle, that the child saw a
    /// CUDA device, that model layers offloaded to the GPU - without
    /// reaching the child's private loopback port. Empty when the config
    /// declares no `[[local_model]]`.
    ///
    /// Available only in builds with the `local` feature.
    #[cfg(feature = "local")]
    pub async fn local_diagnostics(&self) -> Vec<(String, String)> {
        self.state.live.read().await.local.diagnostics()
    }

    /// Serve on a caller-owned listener until `shutdown` completes or
    /// `POST /shutdown` fires the route's own signal, whichever comes
    /// first; both drive the same graceful drain.
    ///
    /// Tests pass an ephemeral [`TcpListener`] they bound themselves (no port
    /// race), read back `local_addr`, and drive a rendezvous instead of
    /// sleeping.
    ///
    /// The command queue's worker task runs for the life of this call: boot
    /// provisioning, profile switches, and unloads all drain through it. A
    /// `Gateway` whose router is taken without serving
    /// ([`router`](Self::router)) has no worker, so its queue accepts but
    /// never runs commands.
    ///
    /// # Errors
    /// Returns [`ServeError`] when the bound address cannot be read or the
    /// HTTP server fails.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServeError> {
        // The configured bind may carry port 0; the bound address is what
        // the host-authority wall allowlists.
        let bound = listener.local_addr().map_err(ServeError::io)?;
        let state = self.state;
        let route_shutdown = state.shutdown.clone();
        let commands = state.commands.clone();
        let commands_after = state.commands.clone();
        let worker = state.commands.spawn_worker(&state);
        // Connect info exposes each request's peer address, so
        // loopback-only routes (`POST /admin/reveal`) can tell loopback
        // callers from LAN callers.
        let result = axum::serve(
            listener,
            build_router(state, Some(bound))
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            tokio::select! {
                () = shutdown => {}
                () = route_shutdown.fired() => {}
            }
            // Cancel the active command before the runtime winds down, so a
            // quit during provisioning stops the download instead of waiting
            // on it.
            commands.cancel_active();
        })
        .await
        .map_err(ServeError::io);
        // Close the queue and reap the worker, so a gateway served on a
        // caller-owned runtime (tests, embedders) leaves no task behind.
        commands_after.shutdown();
        if let Some(worker) = worker {
            let _ = worker.await;
        }
        result
    }
}

#[cfg(test)]
#[cfg(not(feature = "stt"))]
mod stt_tests {
    use super::*;

    #[test]
    fn a_headless_gateway_refuses_an_active_stt_model() {
        let catalog = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
             [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\n\
             source = \"missing.bin\"\nvram_gb = 1.0\n\
             [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n",
        )
        .expect("catalog parses");
        let config = catalog
            .select_profile(&ProfileName::parse("work").expect("profile name"))
            .expect("profile selects");
        let error = Gateway::from_config(&config, ProfilesContext::default())
            .expect_err("STT without the runtime feature must be refused");
        let detail = std::error::Error::source(&error)
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            detail.contains("`stt` feature"),
            "the refusal names the missing feature: {error}: {detail}"
        );
    }
}

/// A running gateway on its own thread, returned by [`spawn`].
///
/// Dropping the handle without calling [`GatewayHandle::shutdown`] still
/// signals the server to stop, but does not wait for it.
#[derive(Debug)]
pub struct GatewayHandle {
    url: String,
    /// The process-lifetime bearer key, captured at bind for the tray's
    /// `/auth` browser-handoff URL. `[server]` edits are restart-required,
    /// so the key cannot change under a running process. Held as a
    /// `Secret` so the derived `Debug` redacts it.
    api_key: Secret,
    /// A clone of the assembled state (all `Arc`s), so the tray's timer
    /// reads model status in-process instead of polling over HTTP.
    state: AppState,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<Result<(), StartupError>>>,
}

impl GatewayHandle {
    /// Returns the base URL of the bound gateway address, for example
    /// `http://127.0.0.1:8081`.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The bearer key the tray needs for its browser-handoff URL.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub(crate) fn tray_key(&self) -> &str {
        self.api_key.expose()
    }

    /// The assembled state, for the tray's in-process status reads.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub(crate) fn tray_state(&self) -> &AppState {
        &self.state
    }

    /// Whether the gateway thread is still serving. A finished thread means
    /// serving ended - requested (`POST /shutdown` fires the shared signal)
    /// or failed; the tray tells the two apart through the signal.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub(crate) fn is_serving(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    /// Signals graceful shutdown and waits for the gateway thread to finish.
    ///
    /// The active queue command's token fires first, so a quit during
    /// provisioning cancels the download and the join returns promptly.
    ///
    /// # Errors
    /// Returns [`StartupError`] when serving failed or the gateway thread
    /// panicked.
    pub fn shutdown(mut self) -> Result<(), StartupError> {
        self.state.commands.cancel_active();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.join_inner()
    }

    /// Waits for the gateway thread to finish on its own, without signaling
    /// shutdown.
    ///
    /// # Errors
    /// Returns [`StartupError`] when serving failed or the gateway thread
    /// panicked.
    pub fn join(mut self) -> Result<(), StartupError> {
        self.join_inner()
    }

    fn join_inner(&mut self) -> Result<(), StartupError> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        match thread.join() {
            Ok(result) => result,
            Err(_) => Err(StartupError::serve(crate::api_error::ServeError::io(
                std::io::Error::other("gateway thread panicked"),
            ))),
        }
    }
}

impl Drop for GatewayHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

/// What the gateway thread reports through the readiness channel: the
/// bound gateway URL, the bearer key, and a clone of the assembled state.
#[derive(Debug)]
struct Ready {
    url: String,
    api_key: Secret,
    state: AppState,
}

/// Spawns the gateway on a dedicated thread and blocks until the listener
/// is bound.
///
/// Config loading, provisioning, and binding all run on the gateway thread;
/// their failures are reported back through this call's return value. The
/// bound listener is the readiness signal: when this returns `Ok`, the
/// gateway is accepting connections at [`GatewayHandle::url`].
///
/// # Errors
/// Returns [`StartupError`] when config loading, provisioning, binding, or
/// starting the gateway thread fails; classify with [`StartupError::kind`].
///
/// # Examples
/// ```no_run
/// use gateway::{ProfileName, ServeOptions, spawn};
/// use std::path::PathBuf;
///
/// let options = ServeOptions::new(
///     Some(PathBuf::from("/etc/promptforge/gateway.toml")),
///     ProfileName::parse("dev").unwrap(),
/// );
/// let gateway = spawn(&options)?;
/// println!("serving on {}", gateway.url());
/// gateway.shutdown()?;
/// # Ok::<(), gateway::StartupError>(())
/// ```
pub fn spawn(options: &ServeOptions) -> Result<GatewayHandle, StartupError> {
    let browser = options.browser;
    let (ready_tx, ready_rx) = mpsc::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let options = options.clone();
    let thread = std::thread::Builder::new()
        .name("gateway".to_string())
        .spawn(move || serve_thread(&options, &ready_tx, shutdown_rx))
        .map_err(StartupError::thread)?;
    match ready_rx.recv() {
        Ok(Ok(ready)) => {
            let handle = GatewayHandle {
                url: ready.url,
                api_key: ready.api_key,
                state: ready.state,
                shutdown: Some(shutdown_tx),
                thread: Some(thread),
            };
            if browser {
                open_settings_page(&handle);
            }
            Ok(handle)
        }
        Ok(Err(error)) => Err(failed_handshake(thread, Some(error))),
        Err(_) => Err(failed_handshake(thread, None)),
    }
}

/// Opens the Settings handoff URL in the default browser, for the binary's
/// `--browser`: once, right after the bind, through the one-time
/// `/auth` redirect so the key never sits in browser history. A browser
/// that cannot launch warns; the gateway serves on.
fn open_settings_page(handle: &GatewayHandle) {
    let url = crate::handoff::auth_url(handle.url(), handle.api_key.expose());
    if let Err(error) = open::that(&url) {
        tracing::warn!("could not open the browser: {error}; the Settings URL is {url}");
    }
}

/// The error [`spawn`] returns after the readiness handshake fails. The
/// gateway thread is joined first, and a panic payload is downcast into
/// the error text: a panicked thread is the one way the readiness channel
/// closes with no message ([`serve_thread`] reports every early exit
/// through it), and a discarded join result would lose the message of the
/// very panic that broke the handshake.
fn failed_handshake(
    thread: JoinHandle<Result<(), StartupError>>,
    reported: Option<StartupError>,
) -> StartupError {
    match thread.join() {
        Ok(_) => reported.unwrap_or_else(|| {
            StartupError::thread(std::io::Error::other(
                "gateway thread exited before binding without reporting an error",
            ))
        }),
        Err(payload) => {
            // `&payload` would unsize the Box itself into `dyn Any`,
            // hiding the real payload type from the downcasts.
            let panic = panic_message(&*payload);
            let message = match &reported {
                Some(error) => format!("{error}; the gateway thread then panicked: {panic}"),
                None => format!("gateway thread panicked before binding: {panic}"),
            };
            StartupError::thread(std::io::Error::other(message))
        }
    }
}

/// The message carried by a panic payload: the `panic!` string when there
/// is one, or a note that the payload is not a string (a `panic_any`
/// call), which carries no displayable message.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

/// The gateway thread's body: load config, build a runtime, bind, assemble
/// the shell, signal readiness through `ready`, post the boot command, then
/// serve until `shutdown` resolves.
///
/// The bind is the readiness signal: provisioning is the boot command's
/// work, queued after the signal fires, so the gateway is reachable in under
/// a second and a quit during provisioning cancels the download. Startup
/// failures are reported through `ready`; only serving failures become the
/// thread's return value.
fn serve_thread(
    options: &ServeOptions,
    ready: &mpsc::Sender<Result<Ready, StartupError>>,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), StartupError> {
    let (config, profiles) = match load_startup(options) {
        Ok(loaded) => loaded,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    let bind = config.bind_addr();
    let hub = Arc::new(shared_progress::ProgressHub::new());
    // The renderer starts before serving so the boot command's downloads
    // log; it is a plain thread, and its Drop stops it on every exit path.
    let _renderer = crate::render::Renderer::start(&hub);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(StartupError::thread(error)));
            return Ok(());
        }
    };
    // The bind runs on this plain thread, apart from the serve future, so
    // the bound address is known before serving starts: the readiness
    // signal and the connection file both carry the real port.
    let listener = match runtime.block_on(TcpListener::bind(bind)) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(StartupError::bind(error)));
            return Ok(());
        }
    };
    // The configured bind may carry port 0; the bound local_addr is the
    // real address the readiness signal must report.
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => {
            let _ = ready.send(Err(StartupError::bind(error)));
            return Ok(());
        }
    };
    // The shell assembles instantly: an empty routing table and no local
    // runtime. Provisioning is the boot command's work, not startup's.
    let boot_profile = profiles.active.clone();
    let gateway = Gateway::new_with_hub(&config, profiles, hub);
    // The connection file lands before the readiness signal so a spawned
    // gateway is discoverable the moment `spawn` returns; the guard removes
    // it on every exit path below, graceful shutdown included.
    let _connection_file = connection_file_guard(&config, options, address);
    tracing::info!("gateway serving on {address}");
    let _ = ready.send(Ok(Ready {
        url: format!("http://{address}"),
        api_key: config.server_key(),
        state: gateway.state.clone(),
    }));
    // The boot command lands after the readiness signal: the queue worker
    // loads the active profile's models into the live routing table while
    // the gateway is already reachable. `persist: false` keeps a
    // command-line or environment profile override ephemeral, exactly as
    // startup always behaved.
    if let Some(name) = boot_profile {
        let _boot = gateway
            .state
            .commands
            .enqueue(crate::commands::Command::load_profile(
                name,
                false,
                tokio_util::sync::CancellationToken::new(),
            ));
    }
    runtime
        .block_on(gateway.serve(listener, shutdown_on_send(shutdown)))
        .map_err(StartupError::serve)
}

/// Removes the connection file on drop when it still belongs to this
/// process, so a graceful shutdown withdraws the gateway from discovery
/// while a replacement's file is spared.
#[derive(Debug)]
struct ConnectionFileGuard {
    run_dir: PathBuf,
    pid: u32,
}

impl Drop for ConnectionFileGuard {
    fn drop(&mut self) {
        if let Err(error) = shared_sidecar::remove_if_mine(&self.run_dir, self.pid) {
            tracing::warn!("could not remove the connection file: {error}");
        }
    }
}

/// Writes the `gateway.json` connection file for the just-bound `address`
/// and returns the guard that removes it on drop. A failure is logged and
/// tolerated: the gateway keeps serving, and discovery degrades to a
/// relaunch instead of an attach.
fn connection_file_guard(
    config: &Config,
    options: &ServeOptions,
    address: std::net::SocketAddr,
) -> Option<ConnectionFileGuard> {
    let Some(run_dir) = options
        .run_dir
        .clone()
        .or_else(shared_sidecar::default_run_dir)
    else {
        tracing::warn!("no user profile directory found; no connection file written");
        return None;
    };
    let now = time::OffsetDateTime::now_utc();
    let file = shared_sidecar::ConnectionFile {
        port: address.port(),
        api_key: config.server_key().expose().to_owned(),
        pid: std::process::id(),
        epoch: u64::try_from(now.unix_timestamp()).unwrap_or(0),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        // A well-known Rfc3339 format of a valid `now_utc` cannot fail;
        // the fallback keeps the field a plain string.
        started_at: now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| String::from("unknown")),
    };
    let pid = file.pid;
    if let Err(error) = file.write_to(&run_dir) {
        tracing::warn!(
            "could not write the connection file in {}: {error}; attachers will relaunch instead",
            run_dir.display()
        );
        return None;
    }
    Some(ConnectionFileGuard { run_dir, pid })
}

/// Resolves only on an explicit shutdown send. A sender dropped without
/// sending - the Ctrl-C handler thread or its runtime failed on the [`run`]
/// path - must keep the server up, never stop it.
async fn shutdown_on_send(shutdown: tokio::sync::oneshot::Receiver<()>) {
    if shutdown.await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Load config, provision local children, bind, and serve until Ctrl-C.
///
/// A thin wrapper over [`spawn`]: the gateway runs on its own thread, a
/// Ctrl-C handler signals its graceful shutdown, and this call blocks until
/// serving ends. The binary stays a thin arg-parsing shell.
///
/// # Errors
/// Returns [`StartupError`] when config loading, provisioning, binding, or
/// serving fails; classify with [`StartupError::kind`].
pub fn run(options: &ServeOptions) -> Result<(), StartupError> {
    run_headless(spawn(options)?)
}

/// Prints the Settings handoff URL to stdout once the listener is bound,
/// then serves headless until Ctrl-C: the tray-less environment's way to
/// reach the config SPA. The URL is the one-time `/auth` redirect, so what
/// lands on the terminal can be pasted into a browser without leaving the
/// bearer key in its history.
///
/// # Errors
/// Returns [`StartupError`] when config loading, provisioning, binding, or
/// serving fails; classify with [`StartupError::kind`].
pub fn run_printing_url(options: &ServeOptions) -> Result<(), StartupError> {
    let handle = spawn(options)?;
    // The URL is the machine-readable output of the `--print-url`
    // affordance, so it goes to stdout itself, not through the log.
    println!(
        "{}",
        crate::handoff::auth_url(handle.url(), handle.api_key.expose())
    );
    run_headless(handle)
}

/// The headless main loop: Ctrl-C signals the gateway's graceful shutdown
/// and this call blocks until serving ends. Shared by [`run`] and the
/// tray's fallback when the system tray cannot start.
///
/// # Errors
/// Returns [`StartupError`] when serving fails or the gateway thread
/// panicked.
pub(crate) fn run_headless(mut handle: GatewayHandle) -> Result<(), StartupError> {
    if let Some(shutdown) = handle.shutdown.take() {
        install_ctrl_c_handler(shutdown);
    }
    handle.join()
}

/// Installs the Ctrl-C handler on its own thread: a genuine interrupt sends
/// the gateway's graceful-shutdown signal, while every failure path sends
/// nothing - [`shutdown_signal`] never resolves on a handler-install
/// failure, and a thread or runtime that fails to start merely drops the
/// sender, which [`shutdown_on_send`] ignores - so no failure can
/// masquerade as an interrupt and stop the gateway.
fn install_ctrl_c_handler(shutdown: tokio::sync::oneshot::Sender<()>) {
    let handler = std::thread::Builder::new()
        .name("gateway-ctrl-c".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(
                        "failed to build the Ctrl-C signal runtime: {error}; continuing to serve"
                    );
                    return;
                }
            };
            runtime.block_on(shutdown_signal());
            let _ = shutdown.send(());
        });
    if let Err(error) = handler {
        tracing::error!(
            "failed to spawn the Ctrl-C handler thread: {error}; the gateway continues to serve"
        );
    }
}

/// What awaiting the Ctrl-C signal produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownTrigger {
    /// A genuine interrupt was received; shut down gracefully.
    Interrupted,
    /// The signal handler could not be installed; do not spuriously shut down.
    HandlerFailed,
}

/// Classifies the result of awaiting Ctrl-C, distinguishing a real interrupt
/// from a failure to install the signal handler (MAIN-003).
fn classify_shutdown(result: &std::io::Result<()>) -> ShutdownTrigger {
    match result {
        Ok(()) => ShutdownTrigger::Interrupted,
        Err(_) => ShutdownTrigger::HandlerFailed,
    }
}

/// Resolves only on a genuine Ctrl-C interrupt.
///
/// If the signal handler cannot be installed, the error is logged and this
/// future never resolves, so a handler-install failure does not masquerade as
/// an interrupt and stop the server. The process can still be terminated by the
/// OS.
async fn shutdown_signal() {
    match classify_shutdown(&tokio::signal::ctrl_c().await) {
        ShutdownTrigger::Interrupted => {
            tracing::info!("received Ctrl-C; shutting down gracefully");
        }
        ShutdownTrigger::HandlerFailed => {
            tracing::error!("failed to install Ctrl-C handler; continuing to serve");
            std::future::pending::<()>().await;
        }
    }
}

/// Resolves the boot config path, loads the one env file, then the one
/// config file with startup precedence. An absent path in `options` runs
/// boot discovery and, when nothing is found, first-run generation.
fn load_startup(options: &ServeOptions) -> Result<(Config, ProfilesContext), StartupError> {
    let config_path = crate::boot::resolve_boot_config(options.config_path.clone())
        .map_err(StartupError::boot)?;
    load_env_file(&config_path.with_extension("env"));
    let environment = std::env::var("PROMPTFORGE_PROFILE").ok();
    load_startup_with_environment(&config_path, options, environment.as_deref())
}

fn load_startup_with_environment(
    config_path: &Path,
    options: &ServeOptions,
    environment: Option<&str>,
) -> Result<(Config, ProfilesContext), StartupError> {
    let selection = gateway_config::ProfileSelection::new(
        options.profile.as_ref().map(ProfileName::as_str),
        environment,
    );
    let config = Config::load(config_path, &selection).map_err(StartupError::config)?;
    if let Some(warning) = workshop_section_deprecation(&config) {
        tracing::warn!("{warning}");
    }
    let active = config
        .active_profile()
        .map(|profile| ProfileName::parse(profile.name()))
        .transpose()
        .map_err(|error| {
            StartupError::config(gateway_config::ConfigError::validation(error.to_string()))
        })?;
    Ok((
        config,
        ProfilesContext::new(Some(config_path.to_path_buf()), active),
    ))
}

/// The deprecation warning for a boot config carrying a `[workshop]`
/// section, or `None` when the section is absent. The gateway no longer
/// hosts the workshop - the desktop shell embeds the workshop server
/// itself - so the section's `bind` and `open_browser` settings do
/// nothing. The section still parses (an existing config must not fail),
/// and `[workshop.stt]` capture tuning still applies to the STT engine;
/// the warning is what keeps the inert fields from being silently
/// ignored.
fn workshop_section_deprecation(config: &Config) -> Option<&'static str> {
    config.workshop().is_some().then_some(
        "the [workshop] section is deprecated: the gateway hosts no workshop listener \
         (the desktop shell embeds the workshop server itself); its bind and open_browser \
         settings are ignored, while [workshop.stt] capture tuning still applies",
    )
}

/// Load an env file into the process environment, skipping missing files.
/// dotenvy never overrides variables that are already set. A malformed or
/// unreadable file is ignored: any variable it failed to set surfaces at
/// interpolation as an unresolved-`${VAR}` error naming the variable.
pub(crate) fn load_env_file(env_path: &Path) {
    if env_path.exists() {
        let _ = dotenvy::from_path(env_path);
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    const CATALOG: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "alpha-model"
description = "alpha"
context = 1024
upstream = "alpha"
endpoints = ["fake"]

[[model]]
name = "beta-model"
description = "beta"
context = 1024
upstream = "beta"
endpoints = ["fake"]

[[profile]]
name = "alpha"
models = ["alpha-model"]

[[profile]]
name = "beta"
models = ["beta-model"]
"#;

    fn fixture(state: &str) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let path = temp.path().join("gateway.toml");
        std::fs::write(&path, CATALOG).expect("write config");
        std::fs::write(
            gateway_config::profile_state_path(&path),
            format!("active_profile = \"{state}\"\n"),
        )
        .expect("write state");
        (temp, path)
    }

    fn error_text(error: &StartupError) -> String {
        let mut text = error.to_string();
        let mut source = error.source();
        while let Some(cause) = source {
            text.push_str("; ");
            text.push_str(&cause.to_string());
            source = cause.source();
        }
        text
    }

    #[test]
    fn command_line_profile_overrides_environment_and_state() {
        let (_temp, path) = fixture("alpha");
        let options = ServeOptions::new(
            Some(path.clone()),
            ProfileName::parse("beta").expect("name"),
        );

        let (config, context) =
            load_startup_with_environment(&path, &options, Some("alpha")).expect("startup loads");

        assert_eq!(config.models()[0].name(), "beta-model");
        assert_eq!(
            context.active.as_ref().map(ProfileName::as_str),
            Some("beta")
        );
    }

    #[test]
    fn environment_profile_overrides_state_without_a_cli_value() {
        let (_temp, path) = fixture("alpha");
        let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);

        let (config, _) =
            load_startup_with_environment(&path, &options, Some("beta")).expect("startup loads");

        assert_eq!(config.models()[0].name(), "beta-model");
    }

    #[test]
    fn startup_uses_the_sibling_state_without_overrides() {
        let (_temp, path) = fixture("alpha");
        let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);

        let (config, context) =
            load_startup_with_environment(&path, &options, None).expect("startup loads");

        assert_eq!(config.models()[0].name(), "alpha-model");
        assert_eq!(context.config_path, Some(path));
    }

    #[test]
    fn unknown_override_lists_the_loaded_catalog_profiles() {
        let (_temp, path) = fixture("alpha");
        let options = ServeOptions::new(
            Some(path.clone()),
            ProfileName::parse("ghost").expect("name"),
        );

        let error = load_startup_with_environment(&path, &options, None)
            .expect_err("unknown profile fails");
        let text = error_text(&error);

        assert!(text.contains("ghost"), "{text}");
        assert!(text.contains("alpha") && text.contains("beta"), "{text}");
    }

    #[test]
    fn a_workshop_section_still_loads_and_earns_the_deprecation_warning() {
        // Existing configs carry `[workshop]` from the hosted-workshop
        // era; they must keep parsing, with the warning discharging the
        // no-silent-ignore rule for the now-inert hosting fields.
        let temp = tempfile::TempDir::new().expect("temp dir");
        let path = temp.path().join("gateway.toml");
        std::fs::write(
            &path,
            format!("{CATALOG}\n[workshop]\nbind = \"127.0.0.1:7910\"\nopen_browser = true\n"),
        )
        .expect("write config");
        std::fs::write(
            gateway_config::profile_state_path(&path),
            "active_profile = \"alpha\"\n",
        )
        .expect("write state");
        let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);

        let (config, _) = load_startup_with_environment(&path, &options, None)
            .expect("a config carrying [workshop] still loads");

        let warning = workshop_section_deprecation(&config)
            .expect("a carried [workshop] section earns the deprecation warning");
        assert!(
            warning.contains("[workshop]"),
            "the warning names the section: {warning}"
        );
        assert!(
            warning.contains("[workshop.stt]"),
            "the warning names what still applies: {warning}"
        );
    }

    #[test]
    fn no_workshop_section_earns_no_deprecation_warning() {
        let (_temp, path) = fixture("alpha");
        let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);
        let (config, _) =
            load_startup_with_environment(&path, &options, None).expect("startup loads");
        assert!(workshop_section_deprecation(&config).is_none());
    }
}
