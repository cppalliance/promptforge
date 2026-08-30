//! Application entry points: [`spawn`], [`run`], and the assembled [`Gateway`].
//!
//! [`spawn`] is the embedding seam: it loads configuration, provisions local
//! children, and serves on a dedicated thread with its own tokio runtime, so
//! an embedding binary keeps its main thread. The call blocks until the
//! listener is bound - that bind is the readiness signal - and the returned
//! [`GatewayHandle`] carries the bound URL and a graceful-shutdown switch.
//! [`run`] is the binary path: a thin wrapper that spawns, installs the
//! Ctrl-C handler, and joins. `Gateway` is the in-process assembly seam used
//! by both and by integration tests, which bind their own listener and drive
//! [`Gateway::serve`] with a caller-owned shutdown signal.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use tokio::net::TcpListener;

use promptforge_gateway_config::{Config, ConfigError, ProfileName, ServerConfig, WorkshopConfig};

use crate::api_error::{ServeError, StartupError};
#[cfg(feature = "local")]
use crate::local::LocalRuntime;
use crate::routing::Routing;
use crate::workshop::{self, WorkshopHandle};
use crate::{AppState, build_router};

/// Options for running the gateway. Built by the binary from parsed args.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Path to the boot TOML (the catalog); profiles dir is its sibling `profiles/`.
    pub config_path: PathBuf,
    /// The profile to boot into; the initial loaded set.
    pub profile: ProfileName,
}

impl ServeOptions {
    /// Build serve options from the boot config path and the profile name.
    #[must_use]
    pub fn new(config_path: PathBuf, profile: ProfileName) -> ServeOptions {
        ServeOptions {
            config_path,
            profile,
        }
    }
}

/// Optional admin-profile directory plus the active profile name.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct ProfilesContext {
    /// Directory of named profiles, enabling the admin switch routes.
    pub dir: Option<PathBuf>,
    /// The active profile name, reported by `GET /admin/status`.
    pub active: Option<ProfileName>,
}

impl ProfilesContext {
    /// Build a profiles context from an optional directory and active name.
    #[must_use]
    pub fn new(dir: Option<PathBuf>, active: Option<ProfileName>) -> ProfilesContext {
        ProfilesContext { dir, active }
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
    /// Assemble from a validated config. Provisions and starts local models.
    ///
    /// The config's `[server]` and `[workshop]` are retained as the
    /// boot-owned settings; profile switches are checked against them (the
    /// socket, the gateway bearer key, and the hosted workshop's settings
    /// are fixed for the process lifetime).
    ///
    /// # Errors
    /// Returns [`StartupError`] when local provisioning or routing construction
    /// fails.
    ///
    /// # Examples
    /// ```
    /// use promptforge_gateway::{Config, Gateway, ProfilesContext};
    ///
    /// let toml = r#"
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
            Arc::new(promptforge_progress::ProgressHub::new()),
        )
    }

    /// [`from_config`](Self::from_config) over a caller-provided progress
    /// hub, so the serving lifecycle's renderer thread can watch startup
    /// provisioning.
    pub(crate) fn from_config_with_hub(
        config: &Config,
        profiles: ProfilesContext,
        hub: Arc<promptforge_progress::ProgressHub>,
    ) -> Result<Gateway, StartupError> {
        // Startup provisioning is the hub's first operation tree: it lives
        // for the provisioning call and detaches when the tree drops.
        #[cfg(feature = "local")]
        let local = {
            let tree = hub.operation();
            let progress = tree.register("startup", 1.0);
            let started =
                LocalRuntime::start(config, Some(&progress)).map_err(StartupError::provisioning);
            if started.is_ok() {
                progress.complete();
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
        let routing = Routing::from_config(config).map_err(StartupError::config)?;
        #[cfg(feature = "local")]
        let routing = routing
            .merge(local.models().iter().cloned())
            .map_err(StartupError::config)?;
        let state = AppState::from_parts(
            Arc::new(routing),
            config.server_key(),
            #[cfg(feature = "local")]
            local,
            #[cfg(feature = "web-search")]
            config.web_search_config(),
            profiles.dir,
            crate::ProfileSelection {
                name: profiles.active.map(|name| name.to_string()),
                model_allowlist: config.model_allowlist().map(<[String]>::to_vec),
            },
            crate::BootOwned {
                server: config.server().clone(),
                workshop: config.workshop().cloned(),
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
    pub fn router(&self) -> axum::Router {
        build_router(self.state.clone())
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

    /// Serve on a caller-owned listener until `shutdown` completes.
    ///
    /// Tests pass an ephemeral [`TcpListener`] they bound themselves (no port
    /// race), read back `local_addr`, and drive a rendezvous instead of
    /// sleeping.
    ///
    /// # Errors
    /// Returns [`ServeError`] when the HTTP server fails.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServeError> {
        axum::serve(listener, build_router(self.state))
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(ServeError::io)
    }
}

/// A running gateway on its own thread, returned by [`spawn`].
///
/// When the boot config carries a `[workshop]` section and the `workshop`
/// feature is compiled in, the handle also holds the hosted workshop
/// server, reachable at [`GatewayHandle::workshop_url`].
///
/// Dropping the handle without calling [`GatewayHandle::shutdown`] still
/// signals both servers to stop, but does not wait for them.
#[derive(Debug)]
pub struct GatewayHandle {
    url: String,
    workshop: Option<WorkshopHandle>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<Result<(), StartupError>>>,
    #[cfg(test)]
    observer: Option<std::sync::mpsc::Sender<ShutdownStep>>,
}

/// One step in [`GatewayHandle::shutdown`]'s ordering, reported through
/// the [`GatewayHandle::observe_shutdown`] test seam.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownStep {
    /// The hosted workshop finished its bounded drain and is fully stopped.
    WorkshopStopped,
    /// The gateway's graceful-shutdown signal was sent.
    GatewaySignaled,
}

impl GatewayHandle {
    /// Returns the base URL of the bound gateway address, for example
    /// `http://127.0.0.1:8081`.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the base URL of the hosted workshop listener, or `None`
    /// when the gateway hosts no workshop (the `workshop` feature is not
    /// compiled in, or the boot config has no `[workshop]` section).
    #[must_use]
    pub fn workshop_url(&self) -> Option<&str> {
        self.workshop.as_ref().map(WorkshopHandle::url)
    }

    /// Signals graceful shutdown and waits for the gateway thread to finish.
    ///
    /// A hosted workshop stops first - waiting out its own bounded drain -
    /// while the gateway still serves, so the workshop's final gateway
    /// calls never hit a dead socket; only then does the gateway drain.
    ///
    /// # Errors
    /// Returns [`StartupError`] when serving failed or the gateway thread
    /// panicked.
    pub fn shutdown(mut self) -> Result<(), StartupError> {
        if let Some(workshop) = self.workshop.take() {
            workshop.shutdown();
            #[cfg(test)]
            self.record(ShutdownStep::WorkshopStopped);
        }
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
            #[cfg(test)]
            self.record(ShutdownStep::GatewaySignaled);
        }
        self.join_inner()
    }

    /// Test seam: records the shutdown sequence, so a test can assert
    /// that a hosted workshop is fully stopped before the gateway's own
    /// shutdown is signaled.
    #[cfg(test)]
    pub(crate) fn observe_shutdown(&mut self, observer: std::sync::mpsc::Sender<ShutdownStep>) {
        self.observer = Some(observer);
    }

    #[cfg(test)]
    fn record(&self, step: ShutdownStep) {
        if let Some(observer) = &self.observer {
            let _ = observer.send(step);
        }
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
        // Same order as shutdown(): the workshop's stop is signaled before
        // the gateway's, though drop waits for neither.
        drop(self.workshop.take());
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

/// What the gateway thread reports through the readiness channel: the
/// bound gateway URL, plus the hosted workshop's handle when the boot
/// config asked for one.
#[derive(Debug)]
struct Ready {
    url: String,
    workshop: Option<WorkshopHandle>,
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
/// use promptforge_gateway::{ProfileName, ServeOptions, spawn};
/// use std::path::PathBuf;
///
/// let options = ServeOptions::new(
///     PathBuf::from("/etc/promptforge/gateway.toml"),
///     ProfileName::parse("dev").unwrap(),
/// );
/// let gateway = spawn(&options)?;
/// println!("serving on {}", gateway.url());
/// gateway.shutdown()?;
/// # Ok::<(), promptforge_gateway::StartupError>(())
/// ```
pub fn spawn(options: &ServeOptions) -> Result<GatewayHandle, StartupError> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let options = options.clone();
    let thread = std::thread::Builder::new()
        .name("promptforge-gateway".to_string())
        .spawn(move || serve_thread(&options, &ready_tx, shutdown_rx))
        .map_err(StartupError::thread)?;
    match ready_rx.recv() {
        Ok(Ok(ready)) => Ok(GatewayHandle {
            url: ready.url,
            workshop: ready.workshop,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
            #[cfg(test)]
            observer: None,
        }),
        Ok(Err(error)) => Err(failed_handshake(thread, Some(error))),
        Err(_) => Err(failed_handshake(thread, None)),
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

/// The gateway thread's body: load config, provision, build a runtime, bind,
/// signal readiness through `ready`, then serve until `shutdown` resolves.
///
/// Startup failures are reported through `ready`; only serving failures
/// become the thread's return value.
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
    let hub = Arc::new(promptforge_progress::ProgressHub::new());
    // The renderer starts before provisioning so boot downloads draw; it is
    // a plain thread because provisioning runs before the runtime exists,
    // and its Drop stops it on every exit path below.
    let _renderer = crate::render::Renderer::start(&hub);
    let gateway = match Gateway::from_config_with_hub(&config, profiles, hub) {
        Ok(gateway) => gateway,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
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
    // The bind runs apart from the serve future so the workshop can start
    // between the two: after the gateway listener exists (a failed gateway
    // bind must not leave a workshop running) and on this plain thread,
    // where the workshop's blocking startup stalls no executor.
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
    let workshop = match workshop::spawn_if_configured(&config, &options.config_path, address) {
        Ok(workshop) => workshop,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    tracing::info!("promptforge-gateway serving on {address}");
    let _ = ready.send(Ok(Ready {
        url: format!("http://{address}"),
        workshop,
    }));
    runtime
        .block_on(gateway.serve(listener, shutdown_on_send(shutdown)))
        .map_err(StartupError::serve)
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
    let mut handle = spawn(options)?;
    if let Some(shutdown) = handle.shutdown.take() {
        install_ctrl_c_handler(handle.workshop.take(), shutdown);
    }
    handle.join()
}

/// Installs the Ctrl-C handler on its own thread: a genuine interrupt stops
/// a hosted workshop first (bounded by the workshop's own drain watchdog)
/// and then sends the gateway's graceful-shutdown signal, while every
/// failure path sends nothing - [`shutdown_signal`] never resolves on a
/// handler-install failure, and a thread or runtime that fails to start
/// merely drops the sender, which [`shutdown_on_send`] ignores - so no
/// failure can masquerade as an interrupt and stop the gateway.
fn install_ctrl_c_handler(
    workshop: Option<WorkshopHandle>,
    shutdown: tokio::sync::oneshot::Sender<()>,
) {
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
                    // Leak the workshop handle: dropping it would signal a
                    // workshop stop, and a signal-runtime failure must leave
                    // both listeners serving until the process is killed.
                    std::mem::forget(workshop);
                    return;
                }
            };
            runtime.block_on(shutdown_signal());
            if let Some(workshop) = workshop {
                workshop.shutdown();
            }
            let _ = shutdown.send(());
        });
    if let Err(error) = handler {
        // The dropped closure signals a hosted workshop's stop, so only the
        // gateway listener is certain to continue.
        tracing::error!(
            "failed to spawn the Ctrl-C handler thread: {error}; the gateway continues to serve \
             (a hosted workshop may stop)"
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

/// The profiles directory for a boot config path: its sibling `profiles/`.
/// A bare filename resolves to `./profiles`.
pub(crate) fn profiles_dir_for(config_path: &Path) -> PathBuf {
    config_path.parent().map_or_else(
        || PathBuf::from("profiles"),
        |parent| parent.join("profiles"),
    )
}

/// The boot-owned `[server]` rule: a profile's merged `[server]` must equal
/// the boot file's `[server]` as values after interpolation, so the socket
/// and the gateway bearer key are fixed for the process lifetime.
///
/// `bind` mismatches name both addresses; `api_key` mismatches report the
/// difference without printing either secret.
pub(crate) fn check_server_matches_boot(
    boot: &ServerConfig,
    candidate: &ServerConfig,
    profile: &ProfileName,
) -> Result<(), ConfigError> {
    if candidate.bind() != boot.bind() {
        return Err(ConfigError::validation(format!(
            "profile {profile} [server] bind mismatch: profile has {}, boot file has {}",
            candidate.bind(),
            boot.bind()
        )));
    }
    if candidate.api_key().expose() != boot.api_key().expose() {
        return Err(ConfigError::validation(format!(
            "profile {profile} [server] api_key mismatch: the profile's key differs from the boot file's (both values redacted)"
        )));
    }
    Ok(())
}

/// The boot-only `[workshop]` rule: like `[server]`, the section lives in
/// the boot config, and a profile's merged `[workshop]` must equal the boot
/// file's as values - present with equal settings, or absent on both sides.
/// The hosted workshop is started once at boot, so a switch can never move,
/// reconfigure, or remove it mid-run.
pub(crate) fn check_workshop_matches_boot(
    boot: Option<&WorkshopConfig>,
    candidate: Option<&WorkshopConfig>,
    profile: &ProfileName,
) -> Result<(), ConfigError> {
    match (boot, candidate) {
        (None, None) => Ok(()),
        (Some(boot), Some(candidate)) if boot == candidate => Ok(()),
        (Some(boot), Some(candidate)) => Err(ConfigError::validation(format!(
            "profile {profile} [workshop] mismatch: {} ([workshop] is boot-only)",
            first_workshop_difference(boot, candidate)
        ))),
        (None, Some(_)) => Err(ConfigError::validation(format!(
            "profile {profile} carries a [workshop] section but the boot file has none \
             ([workshop] is boot-only)"
        ))),
        (Some(_), None) => Err(ConfigError::validation(format!(
            "profile {profile} lacks the boot file's [workshop] section ([workshop] is \
             boot-only; include the boot file or replicate the section)"
        ))),
    }
}

/// Names the first `[workshop]` field that differs between the boot file
/// and the profile, with both values, mirroring
/// [`check_server_matches_boot`]. Only called when `boot != candidate`,
/// so the fallback is unreachable until the config grows a field this
/// check does not name yet.
fn first_workshop_difference(boot: &WorkshopConfig, candidate: &WorkshopConfig) -> String {
    if candidate.bind() != boot.bind() {
        format!(
            "bind mismatch: profile has {}, boot file has {}",
            candidate.bind(),
            boot.bind()
        )
    } else if candidate.open_browser() != boot.open_browser() {
        format!(
            "open_browser mismatch: profile sets {}, boot file sets {}",
            candidate.open_browser(),
            boot.open_browser()
        )
    } else if candidate.voice() != boot.voice() {
        format!(
            "voice mismatch: profile has {:?}, boot file has {:?}",
            candidate.voice(),
            boot.voice()
        )
    } else if candidate.tape() != boot.tape() {
        format!(
            "tape mismatch: profile has {:?}, boot file has {:?}",
            candidate.tape(),
            boot.tape()
        )
    } else {
        "the settings differ in a field this check does not name yet".to_string()
    }
}

/// Boot into the named profile and build the admin profiles context.
///
/// Order matters: the two env files load first (the profile's `<name>.env`,
/// then the boot file's sibling env file; dotenvy never overrides, so the
/// earlier file wins and both lose to the process environment), then the
/// profile resolves with its include chain, then the boot file's `[server]`
/// and `[workshop]` sections are extracted in one pass and compared. The
/// resolved chain is logged, with a warning when the boot file is not in it
/// (the likely-mistake case: an operator edits the boot file and nothing
/// changes).
fn load_startup(options: &ServeOptions) -> Result<(Config, ProfilesContext), StartupError> {
    let profiles_dir = profiles_dir_for(&options.config_path);

    let profile_path = profiles_dir.join(format!("{}.toml", options.profile));
    load_env_file(&profile_path.with_extension("env"));
    load_env_file(&options.config_path.with_extension("env"));

    let available =
        promptforge_gateway_config::list_profiles(&profiles_dir).map_err(StartupError::config)?;
    let wanted = options.profile.to_string();
    if !available.contains(&wanted) {
        let message = if available.is_empty() {
            format!(
                "unknown profile {wanted}; no profiles found in {}",
                profiles_dir.display()
            )
        } else {
            format!(
                "unknown profile {wanted}; available profiles: {}",
                available.join(", ")
            )
        };
        return Err(StartupError::config(ConfigError::validation(message)));
    }

    let (config, chain) = Config::load_profile_with_chain(&profiles_dir, &options.profile)
        .map_err(StartupError::config)?;
    let (boot_server, boot_workshop) =
        promptforge_gateway_config::load_boot_sections(&options.config_path)
            .map_err(StartupError::config)?;
    check_server_matches_boot(&boot_server, config.server(), &options.profile)
        .map_err(StartupError::config)?;
    check_workshop_matches_boot(boot_workshop.as_ref(), config.workshop(), &options.profile)
        .map_err(StartupError::config)?;

    let rendered = chain
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(" -> ");
    tracing::info!(profile = %options.profile, chain = %rendered, "resolved profile include chain");
    if !chain_contains(&chain, &options.config_path) {
        tracing::warn!(
            "boot file {} is not in the include chain of profile {}; edits to it have no effect",
            options.config_path.display(),
            options.profile
        );
    }

    Ok((
        config,
        ProfilesContext::new(Some(profiles_dir), Some(options.profile.clone())),
    ))
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

/// Whether `path` appears in `chain`, comparing canonicalized paths so a
/// chain entry like `profiles/../gateway.toml` matches a CLI `gateway.toml`.
fn chain_contains(chain: &[PathBuf], path: &Path) -> bool {
    let wanted = canonical(path);
    chain.iter().any(|entry| canonical(entry) == wanted)
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::path::{Path, PathBuf};

    use super::{
        ServeOptions, ShutdownStep, ShutdownTrigger, check_server_matches_boot,
        check_workshop_matches_boot, classify_shutdown, failed_handshake, load_startup,
        panic_message, profiles_dir_for, shutdown_on_send, spawn,
    };
    use crate::api_error::{StartupError, StartupErrorKind};
    use promptforge_gateway_config::{Config, ProfileName};

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn gateway_without_local_models_reports_no_diagnostics() {
        let config = Config::from_toml_str(CATALOG).unwrap();
        let gateway =
            super::Gateway::from_config(&config, super::ProfilesContext::default()).unwrap();
        assert!(gateway.local_diagnostics().await.is_empty());
    }

    /// A catalog declaring one `[[local_model]]`; a headless build must
    /// refuse it rather than silently dropping the model.
    #[cfg(not(feature = "local"))]
    const LOCAL_CATALOG: &str = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "boot-key"

[[local_model]]
name = "q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
"#;

    #[cfg(not(feature = "local"))]
    #[test]
    fn headless_boot_refuses_a_config_declaring_local_models() {
        let config = Config::from_toml_str(LOCAL_CATALOG).unwrap();
        let error = super::Gateway::from_config(&config, super::ProfilesContext::default())
            .expect_err("a headless build must refuse a config declaring local models");
        assert_eq!(error.kind(), StartupErrorKind::Provisioning);
        let source = error.source().expect("the refusal carries its cause");
        assert!(
            source.to_string().contains("lacks the `local` feature"),
            "refusal cause: {source}"
        );
    }

    #[test]
    fn classify_shutdown_distinguishes_interrupt_from_handler_failure() {
        assert_eq!(classify_shutdown(&Ok(())), ShutdownTrigger::Interrupted);
        assert_eq!(
            classify_shutdown(&Err(std::io::Error::other("no handler"))),
            ShutdownTrigger::HandlerFailed
        );
    }

    #[test]
    fn profiles_dir_is_the_boot_files_sibling() {
        assert_eq!(
            profiles_dir_for(Path::new("/etc/pf/gateway.toml")),
            PathBuf::from("/etc/pf/profiles")
        );
        assert_eq!(
            profiles_dir_for(Path::new("gateway.toml")),
            PathBuf::from("profiles"),
            "a bare filename resolves to ./profiles"
        );
    }

    /// A boot catalog that needs no `${VAR}` interpolation.
    const CATALOG: &str = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "boot-key"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// A catalog whose boot key comes from the environment.
    const CATALOG_ENV_KEY: &str = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "${PFG_S4_BOOT_KEY}"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#;

    /// A tempdir holding `gateway.toml` plus an empty `profiles/` dir.
    fn boot_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        write(tmp.path(), "gateway.toml", CATALOG);
        std::fs::create_dir(tmp.path().join("profiles")).unwrap();
        let config_path = tmp.path().join("gateway.toml");
        (tmp, config_path)
    }

    /// The error's Display plus every source in the chain, for assertions.
    fn error_text(error: &crate::api_error::StartupError) -> String {
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
    fn boots_into_a_profile_with_active_reported() {
        let (tmp, config_path) = boot_fixture();
        write(
            &tmp.path().join("profiles"),
            "main.toml",
            "include = [\"../gateway.toml\"]\n",
        );

        let options = ServeOptions::new(config_path, ProfileName::parse("main").unwrap());
        let (config, context) = load_startup(&options).unwrap();
        assert_eq!(config.models()[0].name(), "m");
        assert_eq!(context.active, Some(ProfileName::parse("main").unwrap()));
        assert_eq!(context.dir, Some(tmp.path().join("profiles")));
    }

    #[test]
    fn boot_loads_profile_env_first_so_it_wins_over_boot_env() {
        let tmp = tempfile::TempDir::new().unwrap();
        write(tmp.path(), "gateway.toml", CATALOG_ENV_KEY);
        write(tmp.path(), "gateway.env", "PFG_S4_BOOT_KEY=from-boot-env\n");
        let profiles = tmp.path().join("profiles");
        std::fs::create_dir(&profiles).unwrap();
        write(&profiles, "main.toml", "include = [\"../gateway.toml\"]\n");
        write(&profiles, "main.env", "PFG_S4_BOOT_KEY=from-profile-env\n");

        let options = ServeOptions::new(
            tmp.path().join("gateway.toml"),
            ProfileName::parse("main").unwrap(),
        );
        let (config, _context) = load_startup(&options).unwrap();
        assert_eq!(
            config.server().api_key().expose(),
            "from-profile-env",
            "the profile env loads first and dotenvy never overrides"
        );
    }

    #[test]
    fn self_contained_profile_boots_when_server_matches() {
        // Value equality, not provenance: a profile replicating [server]
        // verbatim passes even though the boot file is not in its chain.
        let (tmp, config_path) = boot_fixture();
        write(&tmp.path().join("profiles"), "solo.toml", CATALOG);

        let options = ServeOptions::new(config_path, ProfileName::parse("solo").unwrap());
        let (config, _context) = load_startup(&options).unwrap();
        assert_eq!(config.models()[0].name(), "m");
    }

    #[test]
    fn server_key_mismatch_fails_boot() {
        let (tmp, config_path) = boot_fixture();
        write(
            &tmp.path().join("profiles"),
            "main.toml",
            "include = [\"../gateway.toml\"]\n\n[server]\napi_key = \"other-key\"\n",
        );

        let options = ServeOptions::new(config_path, ProfileName::parse("main").unwrap());
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(text.contains("api_key mismatch"), "got: {text}");
        assert!(!text.contains("other-key"), "secrets stay redacted: {text}");
    }

    #[test]
    fn server_bind_mismatch_fails_boot_naming_both_values() {
        let (tmp, config_path) = boot_fixture();
        write(
            &tmp.path().join("profiles"),
            "main.toml",
            "include = [\"../gateway.toml\"]\n\n[server]\nbind = \"127.0.0.1:9999\"\n",
        );

        let options = ServeOptions::new(config_path, ProfileName::parse("main").unwrap());
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(text.contains("bind mismatch"), "got: {text}");
        assert!(text.contains("127.0.0.1:9999"), "profile value: {text}");
        assert!(text.contains("127.0.0.1:8081"), "boot value: {text}");
    }

    #[test]
    fn unknown_profile_error_lists_available_profiles() {
        let (tmp, config_path) = boot_fixture();
        let profiles = tmp.path().join("profiles");
        write(&profiles, "alpha.toml", "include = [\"../gateway.toml\"]\n");
        write(&profiles, "beta.toml", "include = [\"../gateway.toml\"]\n");

        let options = ServeOptions::new(config_path, ProfileName::parse("ghost").unwrap());
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(
            text.contains("ghost"),
            "names the requested profile: {text}"
        );
        assert!(text.contains("alpha"), "lists alpha: {text}");
        assert!(text.contains("beta"), "lists beta: {text}");
    }

    #[test]
    fn missing_profiles_dir_is_a_boot_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        write(tmp.path(), "gateway.toml", CATALOG);

        let options = ServeOptions::new(
            tmp.path().join("gateway.toml"),
            ProfileName::parse("main").unwrap(),
        );
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(text.contains("no profiles found"), "got: {text}");
    }

    #[test]
    fn server_check_compares_values_not_provenance() {
        // Two separately parsed copies of the same [server] are equal as
        // values; no shared origin is required.
        let boot = Config::from_toml_str(CATALOG).unwrap().server().clone();
        let same = Config::from_toml_str(CATALOG).unwrap().server().clone();
        let profile = ProfileName::parse("p").unwrap();
        check_server_matches_boot(&boot, &same, &profile).unwrap();
    }

    /// Parses the catalog plus `extra` and returns its `[workshop]` section.
    fn workshop_of(extra: &str) -> Option<promptforge_gateway_config::WorkshopConfig> {
        Config::from_toml_str(&format!("{CATALOG}{extra}"))
            .unwrap()
            .workshop()
            .cloned()
    }

    #[test]
    fn workshop_check_accepts_equal_or_absent_sections() {
        let profile = ProfileName::parse("p").unwrap();
        check_workshop_matches_boot(None, None, &profile).unwrap();

        let boot = workshop_of("[workshop]\nbind = \"127.0.0.1:7910\"\n");
        let same = workshop_of("[workshop]\nbind = \"127.0.0.1:7910\"\n");
        check_workshop_matches_boot(boot.as_ref(), same.as_ref(), &profile).unwrap();
    }

    #[test]
    fn workshop_check_rejects_a_differing_or_one_sided_section() {
        let profile = ProfileName::parse("p").unwrap();
        let boot = workshop_of("[workshop]\nbind = \"127.0.0.1:7910\"\n");
        let changed = workshop_of("[workshop]\nbind = \"127.0.0.1:7911\"\n");

        let differing =
            check_workshop_matches_boot(boot.as_ref(), changed.as_ref(), &profile).unwrap_err();
        assert!(
            differing.to_string().contains("[workshop] mismatch"),
            "got: {differing}"
        );

        let added = check_workshop_matches_boot(None, boot.as_ref(), &profile).unwrap_err();
        assert!(
            added.to_string().contains("boot file has none"),
            "got: {added}"
        );

        let dropped = check_workshop_matches_boot(boot.as_ref(), None, &profile).unwrap_err();
        assert!(
            dropped.to_string().contains("lacks the boot file's"),
            "got: {dropped}"
        );
    }

    #[test]
    fn workshop_mismatch_names_the_first_differing_field() {
        let profile = ProfileName::parse("p").unwrap();
        let boot = workshop_of("[workshop]\nbind = \"127.0.0.1:7910\"\n");

        let changed_bind = workshop_of("[workshop]\nbind = \"127.0.0.1:7911\"\n");
        let error = check_workshop_matches_boot(boot.as_ref(), changed_bind.as_ref(), &profile)
            .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("bind mismatch"), "got: {text}");
        assert!(text.contains("127.0.0.1:7911"), "profile value: {text}");
        assert!(text.contains("127.0.0.1:7910"), "boot value: {text}");

        let changed_open =
            workshop_of("[workshop]\nbind = \"127.0.0.1:7910\"\nopen_browser = true\n");
        let error = check_workshop_matches_boot(boot.as_ref(), changed_open.as_ref(), &profile)
            .unwrap_err();
        assert!(
            error.to_string().contains("open_browser mismatch"),
            "got: {error}"
        );

        let changed_voice = workshop_of(
            "[workshop]\nbind = \"127.0.0.1:7910\"\n\n[workshop.voice]\nwindow_seconds = 8\n",
        );
        let error = check_workshop_matches_boot(boot.as_ref(), changed_voice.as_ref(), &profile)
            .unwrap_err();
        assert!(error.to_string().contains("voice mismatch"), "got: {error}");

        let changed_tape = workshop_of(
            "[workshop]\nbind = \"127.0.0.1:7910\"\n\n[workshop.tape]\npath = \"other.jsonl\"\n",
        );
        let error = check_workshop_matches_boot(boot.as_ref(), changed_tape.as_ref(), &profile)
            .unwrap_err();
        assert!(error.to_string().contains("tape mismatch"), "got: {error}");
    }

    #[test]
    fn boot_accepts_a_profile_inheriting_the_boot_workshop() {
        let tmp = tempfile::TempDir::new().unwrap();
        write(
            tmp.path(),
            "gateway.toml",
            &format!("{CATALOG}[workshop]\nbind = \"127.0.0.1:7910\"\n"),
        );
        let profiles = tmp.path().join("profiles");
        std::fs::create_dir(&profiles).unwrap();
        write(&profiles, "main.toml", "include = [\"../gateway.toml\"]\n");

        let options = ServeOptions::new(
            tmp.path().join("gateway.toml"),
            ProfileName::parse("main").unwrap(),
        );
        let (config, _context) = load_startup(&options).unwrap();
        assert_eq!(
            config
                .workshop()
                .map(|workshop| workshop.bind().to_string()),
            Some("127.0.0.1:7910".to_string())
        );
    }

    #[test]
    fn boot_refuses_a_profile_with_a_differing_workshop() {
        let tmp = tempfile::TempDir::new().unwrap();
        write(
            tmp.path(),
            "gateway.toml",
            &format!("{CATALOG}[workshop]\nbind = \"127.0.0.1:7910\"\n"),
        );
        let profiles = tmp.path().join("profiles");
        std::fs::create_dir(&profiles).unwrap();
        write(
            &profiles,
            "main.toml",
            "include = [\"../gateway.toml\"]\n\n[workshop]\nbind = \"127.0.0.1:7911\"\n",
        );

        let options = ServeOptions::new(
            tmp.path().join("gateway.toml"),
            ProfileName::parse("main").unwrap(),
        );
        let error = load_startup(&options).unwrap_err();
        let text = error_text(&error);
        assert!(text.contains("[workshop] mismatch"), "got: {text}");
    }

    /// A boot catalog on an ephemeral port, so spawn tests never collide.
    const CATALOG_EPHEMERAL: &str = r#"
[server]
bind = "127.0.0.1:0"
api_key = "boot-key"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#;

    /// A tempdir with the given boot catalog and a `main` profile that
    /// includes it, plus the spawn options that boot into it.
    fn spawn_fixture(catalog: &str) -> (tempfile::TempDir, ServeOptions) {
        let tmp = tempfile::TempDir::new().unwrap();
        write(tmp.path(), "gateway.toml", catalog);
        let profiles = tmp.path().join("profiles");
        std::fs::create_dir(&profiles).unwrap();
        write(&profiles, "main.toml", "include = [\"../gateway.toml\"]\n");
        let options = ServeOptions::new(
            tmp.path().join("gateway.toml"),
            ProfileName::parse("main").unwrap(),
        );
        (tmp, options)
    }

    /// A raw HTTP/1.1 GET against `url`, returning the full response text.
    /// Keeps an HTTP client out of the crate's dev-dependencies.
    fn http_get(url: &str, path: &str) -> String {
        use std::io::{Read as _, Write as _};

        let address = url.strip_prefix("http://").expect("the URL is http");
        let mut stream = std::net::TcpStream::connect(address).expect("the gateway accepts");
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: gateway\r\nConnection: close\r\n\r\n"
        )
        .expect("the request sends");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("the response reads");
        response
    }

    #[test]
    fn spawn_readiness_means_the_health_endpoint_answers() {
        let (_tmp, options) = spawn_fixture(CATALOG_EPHEMERAL);
        let gateway = spawn(&options).expect("gateway spawns");
        assert!(
            gateway.url().starts_with("http://127.0.0.1:"),
            "the URL carries the bound loopback address: {}",
            gateway.url()
        );

        let response = http_get(gateway.url(), "/health");
        assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");
        assert!(
            response.contains(r#""status":"serving""#),
            "got: {response}"
        );

        gateway.shutdown().expect("graceful shutdown succeeds");
    }

    #[test]
    fn shutdown_stops_serving_and_releases_the_port() {
        let (_tmp, options) = spawn_fixture(CATALOG_EPHEMERAL);
        let gateway = spawn(&options).expect("gateway spawns");
        let address = gateway
            .url()
            .strip_prefix("http://")
            .expect("the URL is http")
            .to_string();

        gateway.shutdown().expect("graceful shutdown succeeds");

        assert!(
            std::net::TcpStream::connect(&address).is_err(),
            "nothing may accept on {address} after shutdown"
        );
        drop(std::net::TcpListener::bind(&address).expect("the port is free after shutdown"));
    }

    #[test]
    fn dropping_the_handle_signals_shutdown() {
        let (_tmp, options) = spawn_fixture(CATALOG_EPHEMERAL);
        let gateway = spawn(&options).expect("gateway spawns");
        let address = gateway
            .url()
            .strip_prefix("http://")
            .expect("the URL is http")
            .to_string();

        drop(gateway);

        // Drop signals shutdown but does not wait, so poll with a deadline.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::net::TcpStream::connect(&address).is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "the gateway still accepts on {address} after the handle dropped"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn a_dropped_shutdown_sender_never_resolves_the_serve_future() {
        use futures_util::FutureExt as _;

        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        drop(sender);
        assert!(
            shutdown_on_send(receiver).now_or_never().is_none(),
            "a sender dropped without sending (a failed Ctrl-C handler) must not stop the server"
        );
    }

    #[test]
    fn an_explicit_shutdown_send_resolves_the_serve_future() {
        use futures_util::FutureExt as _;

        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        sender.send(()).expect("the receiver is alive");
        assert!(
            shutdown_on_send(receiver).now_or_never().is_some(),
            "an explicit send is the shutdown signal"
        );
    }

    /// The ephemeral catalog plus a `[workshop]` section on its own
    /// ephemeral loopback port.
    fn catalog_with_workshop() -> String {
        format!("{CATALOG_EPHEMERAL}\n[workshop]\nbind = \"127.0.0.1:0\"\n")
    }

    #[test]
    #[cfg(feature = "workshop")]
    fn the_hosted_workshop_serves_and_stops_with_the_gateway() {
        let (_tmp, options) = spawn_fixture(&catalog_with_workshop());
        let gateway = spawn(&options).expect("gateway spawns");
        let workshop_url = gateway
            .workshop_url()
            .expect("a [workshop] boot hosts a workshop")
            .to_string();
        assert!(
            workshop_url.starts_with("http://127.0.0.1:"),
            "the workshop URL carries the bound loopback address: {workshop_url}"
        );

        let response = http_get(&workshop_url, "/health");
        assert!(response.starts_with("HTTP/1.1 200"), "got: {response}");

        let strip = |url: &str| {
            url.strip_prefix("http://")
                .expect("the URL is http")
                .to_string()
        };
        let gateway_address = strip(gateway.url());
        let workshop_address = strip(&workshop_url);
        gateway.shutdown().expect("graceful shutdown succeeds");
        assert!(
            std::net::TcpStream::connect(&workshop_address).is_err(),
            "nothing may accept on the workshop address {workshop_address} after shutdown"
        );
        assert!(
            std::net::TcpStream::connect(&gateway_address).is_err(),
            "nothing may accept on the gateway address {gateway_address} after shutdown"
        );
    }

    #[test]
    fn shutdown_without_a_workshop_signals_the_gateway_only() {
        let (_tmp, options) = spawn_fixture(CATALOG_EPHEMERAL);
        let mut gateway = spawn(&options).expect("gateway spawns");
        let (tx, rx) = std::sync::mpsc::channel();
        gateway.observe_shutdown(tx);
        gateway.shutdown().expect("graceful shutdown succeeds");
        let steps: Vec<ShutdownStep> = rx.try_iter().collect();
        assert_eq!(
            steps,
            [ShutdownStep::GatewaySignaled],
            "no workshop is hosted, so only the gateway signal is recorded"
        );
    }

    #[test]
    #[cfg(feature = "workshop")]
    fn shutdown_drains_the_workshop_before_signaling_the_gateway() {
        let (_tmp, options) = spawn_fixture(&catalog_with_workshop());
        let mut gateway = spawn(&options).expect("gateway spawns");
        assert!(
            gateway.workshop_url().is_some(),
            "a [workshop] boot hosts a workshop"
        );

        let (tx, rx) = std::sync::mpsc::channel();
        gateway.observe_shutdown(tx);
        gateway.shutdown().expect("graceful shutdown succeeds");

        // Both steps are recorded synchronously inside shutdown(), before
        // it returns, so there is nothing to wait for.
        let steps: Vec<ShutdownStep> = rx.try_iter().collect();
        assert_eq!(
            steps,
            [ShutdownStep::WorkshopStopped, ShutdownStep::GatewaySignaled],
            "the workshop's drain completes before the gateway's shutdown is signaled"
        );
    }

    #[test]
    #[cfg(not(feature = "workshop"))]
    fn a_workshop_section_is_ignored_without_the_feature() {
        let (_tmp, options) = spawn_fixture(&catalog_with_workshop());
        let gateway = spawn(&options).expect("gateway spawns without hosting");
        assert!(
            gateway.workshop_url().is_none(),
            "no workshop is hosted without the workshop feature"
        );
        gateway.shutdown().expect("graceful shutdown succeeds");
    }

    #[test]
    fn spawn_returns_config_errors_through_the_handshake() {
        let (_tmp, config_path) = boot_fixture();
        let options = ServeOptions::new(config_path, ProfileName::parse("ghost").unwrap());
        let error = spawn(&options).expect_err("an unknown profile must fail spawn");
        assert_eq!(error.kind(), StartupErrorKind::Config);
    }

    #[test]
    fn a_bind_conflict_fails_spawn_with_bind_kind() {
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blocker");
        let address = blocker.local_addr().expect("blocker address");
        let catalog = CATALOG_EPHEMERAL.replace("127.0.0.1:0", &address.to_string());
        let (_tmp, options) = spawn_fixture(&catalog);
        let error = spawn(&options).expect_err("a taken port must fail spawn");
        assert_eq!(error.kind(), StartupErrorKind::Bind);
    }

    #[test]
    fn a_panicked_gateway_thread_folds_its_panic_message_into_the_error() {
        let thread = std::thread::Builder::new()
            .name("gateway-panic-fixture".to_string())
            .spawn(|| -> Result<(), StartupError> {
                panic!("the gateway thread lost its config");
            })
            .expect("the fixture thread spawns");
        let error = failed_handshake(thread, None);
        assert_eq!(error.kind(), StartupErrorKind::Thread);
        let text = error_text(&error);
        assert!(
            text.contains("the gateway thread lost its config"),
            "the panic message survives the join: {text}"
        );
        assert!(
            !text.contains("failed to bind the listener"),
            "a pre-bind panic is not misnamed a bind failure: {text}"
        );
    }

    #[test]
    fn a_silent_thread_exit_is_thread_kind_not_bind_kind() {
        let thread = std::thread::Builder::new()
            .name("gateway-exit-fixture".to_string())
            .spawn(|| -> Result<(), StartupError> { Ok(()) })
            .expect("the fixture thread spawns");
        let error = failed_handshake(thread, None);
        assert_eq!(error.kind(), StartupErrorKind::Thread);
        let text = error_text(&error);
        assert!(text.contains("exited before binding"), "got: {text}");
    }

    #[test]
    fn a_reported_handshake_error_survives_the_join_unchanged() {
        let thread = std::thread::Builder::new()
            .name("gateway-report-fixture".to_string())
            .spawn(|| -> Result<(), StartupError> { Ok(()) })
            .expect("the fixture thread spawns");
        let reported = StartupError::bind(std::io::Error::other("port taken"));
        let error = failed_handshake(thread, Some(reported));
        assert_eq!(
            error.kind(),
            StartupErrorKind::Bind,
            "the handshake's own error stays primary when the thread exits cleanly"
        );
    }

    #[test]
    fn panic_message_reads_each_payload_shape() {
        assert_eq!(panic_message(&"borrowed"), "borrowed");
        assert_eq!(panic_message(&String::from("owned")), "owned");
        assert_eq!(
            panic_message(&42_u64),
            "non-string panic payload",
            "a panic_any payload carries no displayable message"
        );
    }
}
