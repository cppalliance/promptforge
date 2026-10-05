//! Application entry points: [`spawn`], [`run`], and the assembled [`Gateway`].
//!
//! [`spawn`] is the embedding seam: it loads configuration, assembles the
//! serving shell, and serves on a dedicated thread with its own tokio
//! runtime, so an embedding binary keeps its main thread. The call blocks
//! until the listener is bound - that bind is the readiness signal - and the
//! returned [`GatewayHandle`] holds the bound URL and a graceful-shutdown
//! switch. The remote routing table is published at assembly; local
//! provisioning is not on this path: the boot `LoadProfile` command runs on
//! the gateway's command queue after the bind. [`run`] is the binary
//! path: a thin wrapper that spawns, installs the
//! Ctrl-C handler, and joins. `Gateway` is the in-process assembly seam used
//! by both and by integration tests, which bind their own listener and drive
//! [`Gateway::serve`] with a caller-owned shutdown signal.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use gateway_config::{Config, ProfileName, Secret};

use crate::AppState;
use crate::api_error::StartupError;

mod gateway;
mod thread;

pub use self::thread::spawn;

/// How long a shutdown waits for in-flight requests to finish before it
/// abandons them. Long enough for a response already being written; short
/// enough that a quit never looks stuck. Streams that select on the
/// shutdown signal end at once and never reach this bound.
const GRACEFUL_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long the gateway thread waits for `spawn_blocking` work after
/// serving ends. Dropping a tokio runtime waits indefinitely for its
/// blocking pool, and a request the drain abandoned may still sit in a
/// blocking call, so the runtime is abandoned after this bound and the
/// leftover work dies with the process. The command worker is joined and
/// the speech service retired by `serve` under the one
/// [`WORKER_JOIN_TIMEOUT`] deadline before this teardown runs, so a command
/// body that ignored its token, a speech retirement still draining, or a
/// retirement left unawaited because the join spent the deadline is
/// already abandoned by then; either retirement is blocking-pool work this
/// bound reaps.
const RUNTIME_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long `serve` waits for the command worker after the queue closes.
/// A command body that ignores its cancellation token (a stalled download
/// or spawn) would otherwise pin the join forever; after this bound the
/// worker is abandoned and [`RUNTIME_SHUTDOWN_TIMEOUT`] reaps what is
/// left. No command writes a state file, so the bound can never abandon a
/// half-written one; only a download or a spawn can outlive it. The join
/// and the speech retirement that follows it share this deadline, so the
/// two waits together take at most this long, and a join that spends it
/// leaves the retirement unawaited.
const WORKER_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
    /// Directory the `gateway.json` gateway discovery file is written to after a
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

    /// Sets the gateway discovery file's run directory, for tests and portable
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

/// Resolves only on an explicit shutdown send. A sender dropped without
/// sending - the Ctrl-C handler thread or its runtime failed on the [`run`]
/// path - must keep the server up, never stop it.
async fn shutdown_on_send(shutdown: tokio::sync::oneshot::Receiver<()>) {
    if shutdown.await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Loads config, provisions local children, binds, and serves until Ctrl-C.
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
        crate::auth::primitives::auth_url(handle.url(), handle.api_key.expose())
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

/// What the boot log says about a config that selected no profile.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BootSelectionNotice {
    /// The state file named a profile the config no longer defines; the
    /// load degraded to no profile. Logged as a warning.
    Stale(String),
    /// Nothing selected a profile. Logged for information.
    None(String),
}

/// The notice for a boot config with no selected profile: the stale-state
/// warning naming the missing profile and the defined ones, the plain
/// no-profile line, or `None` when a profile is selected.
fn boot_selection_notice(config: &Config) -> Option<BootSelectionNotice> {
    if config.active_profile().is_some() {
        return None;
    }
    let Some(stale) = config.stale_state_selection() else {
        return Some(BootSelectionNotice::None(
            "no profile selected; serving remote models only".to_owned(),
        ));
    };
    let defined: Vec<&str> = config
        .profiles()
        .iter()
        .map(gateway_config::ProfileConfig::name)
        .collect();
    let defined = if defined.is_empty() {
        "none".to_owned()
    } else {
        defined.join(", ")
    };
    Some(BootSelectionNotice::Stale(format!(
        "state file selects profile \"{stale}\", which is not defined (defined profiles: {defined}); booting with no profile"
    )))
}

/// The deprecation warning for a boot config with a `[workshop]`
/// section, or `None` when the section is absent. The gateway runs no
/// workshop listener - the desktop shell embeds the workshop server
/// itself - so the section's `bind` and `open_browser` settings do
/// nothing. The section parses so a config that includes it does not
/// break startup; the warning keeps those inert fields from being silently
/// ignored.
fn workshop_section_deprecation(config: &Config) -> Option<&'static str> {
    config.workshop().is_some().then_some(
        "the [workshop] section is deprecated: the gateway runs no workshop listener \
         (the desktop shell embeds the workshop server itself); its bind and open_browser \
         settings are ignored",
    )
}

/// Loads an env file into the process environment, skipping missing files.
/// dotenvy never overrides variables that are already set. A malformed or
/// unreadable file is ignored: any variable it failed to set surfaces at
/// interpolation as an unresolved-`${VAR}` error naming the variable.
fn load_env_file(env_path: &Path) {
    if env_path.exists() {
        let _ = dotenvy::from_path(env_path);
    }
}

#[cfg(test)]
#[cfg(not(feature = "stt"))]
mod stt_tests;

#[cfg(test)]
mod drain_tests;

#[cfg(test)]
mod tests;
