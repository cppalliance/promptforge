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

use gateway_config::{Config, ProfileName};

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
    /// Path to the single global configuration file.
    pub config_path: PathBuf,
    /// Optional command-line profile override.
    pub profile: Option<ProfileName>,
}

impl ServeOptions {
    /// Builds serve options from the config path and optional profile override.
    #[must_use]
    pub fn new(config_path: PathBuf, profile: impl Into<Option<ProfileName>>) -> ServeOptions {
        ServeOptions {
            config_path,
            profile: profile.into(),
        }
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
        if !config.stt_models().is_empty() && config.workshop().is_none() {
            return Err(StartupError::provisioning(std::io::Error::other(
                crate::STT_REQUIRES_WORKSHOP,
            )));
        }
        #[cfg(feature = "workshop")]
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
        #[cfg(not(feature = "workshop"))]
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
            #[cfg(feature = "workshop")]
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
        // Connect info exposes each request's peer address, so
        // loopback-only routes (`POST /admin/reveal`) can tell loopback
        // callers from LAN callers.
        axum::serve(
            listener,
            build_router(self.state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(ServeError::io)
    }
}

#[cfg(test)]
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
            .expect_err("STT without a workshop listener must be refused");
        let detail = std::error::Error::source(&error)
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            detail.contains("[workshop]"),
            "the refusal names the missing listener section: {error}: {detail}"
        );
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
        }
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
/// use gateway::{ProfileName, ServeOptions, spawn};
/// use std::path::PathBuf;
///
/// let options = ServeOptions::new(
///     PathBuf::from("/etc/promptforge/gateway.toml"),
///     ProfileName::parse("dev").unwrap(),
/// );
/// let gateway = spawn(&options)?;
/// println!("serving on {}", gateway.url());
/// gateway.shutdown()?;
/// # Ok::<(), gateway::StartupError>(())
/// ```
pub fn spawn(options: &ServeOptions) -> Result<GatewayHandle, StartupError> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let options = options.clone();
    let thread = std::thread::Builder::new()
        .name("gateway".to_string())
        .spawn(move || serve_thread(&options, &ready_tx, shutdown_rx))
        .map_err(StartupError::thread)?;
    match ready_rx.recv() {
        Ok(Ok(ready)) => Ok(GatewayHandle {
            url: ready.url,
            workshop: ready.workshop,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
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
    let hub = Arc::new(shared_progress::ProgressHub::new());
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
    #[cfg(feature = "workshop")]
    let workshop = workshop::spawn_if_configured(
        &config,
        &options.config_path,
        address,
        gateway.state.stt_state(),
    );
    #[cfg(not(feature = "workshop"))]
    let workshop = workshop::spawn_if_configured(&config, &options.config_path, address);
    let workshop = match workshop {
        Ok(workshop) => workshop,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    tracing::info!("gateway serving on {address}");
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

/// Loads the one env file, then the one config file with startup precedence.
fn load_startup(options: &ServeOptions) -> Result<(Config, ProfilesContext), StartupError> {
    load_env_file(&options.config_path.with_extension("env"));
    let environment = std::env::var("PROMPTFORGE_PROFILE").ok();
    load_startup_with_environment(options, environment.as_deref())
}

fn load_startup_with_environment(
    options: &ServeOptions,
    environment: Option<&str>,
) -> Result<(Config, ProfilesContext), StartupError> {
    let selection = gateway_config::ProfileSelection::new(
        options.profile.as_ref().map(ProfileName::as_str),
        environment,
    );
    let config = Config::load(&options.config_path, &selection).map_err(StartupError::config)?;
    let active = config
        .active_profile()
        .map(|profile| ProfileName::parse(profile.name()))
        .transpose()
        .map_err(|error| {
            StartupError::config(gateway_config::ConfigError::validation(error.to_string()))
        })?;
    Ok((
        config,
        ProfilesContext::new(Some(options.config_path.clone()), active),
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
        let options = ServeOptions::new(path, ProfileName::parse("beta").expect("name"));

        let (config, context) =
            load_startup_with_environment(&options, Some("alpha")).expect("startup loads");

        assert_eq!(config.models()[0].name(), "beta-model");
        assert_eq!(
            context.active.as_ref().map(ProfileName::as_str),
            Some("beta")
        );
    }

    #[test]
    fn environment_profile_overrides_state_without_a_cli_value() {
        let (_temp, path) = fixture("alpha");
        let options = ServeOptions::new(path, None::<ProfileName>);

        let (config, _) =
            load_startup_with_environment(&options, Some("beta")).expect("startup loads");

        assert_eq!(config.models()[0].name(), "beta-model");
    }

    #[test]
    fn startup_uses_the_sibling_state_without_overrides() {
        let (_temp, path) = fixture("alpha");
        let options = ServeOptions::new(path, None::<ProfileName>);

        let (config, context) =
            load_startup_with_environment(&options, None).expect("startup loads");

        assert_eq!(config.models()[0].name(), "alpha-model");
        assert_eq!(context.config_path, Some(options.config_path));
    }

    #[test]
    fn unknown_override_lists_the_loaded_catalog_profiles() {
        let (_temp, path) = fixture("alpha");
        let options = ServeOptions::new(path, ProfileName::parse("ghost").expect("name"));

        let error =
            load_startup_with_environment(&options, None).expect_err("unknown profile fails");
        let text = error_text(&error);

        assert!(text.contains("ghost"), "{text}");
        assert!(text.contains("alpha") && text.contains("beta"), "{text}");
    }
}
