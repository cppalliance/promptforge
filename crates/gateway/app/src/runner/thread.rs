//! The gateway thread behind [`spawn`]: the readiness handshake, the
//! thread's serve body, the gateway discovery file, and the
//! [`GatewayHandle`] the caller holds.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use tokio::net::TcpListener;

use gateway_config::{Config, Secret};

use super::{
    BootSelectionNotice, Gateway, GatewayHandle, RUNTIME_SHUTDOWN_TIMEOUT, ServeOptions,
    boot_selection_notice, load_startup, shutdown_on_send,
};
use crate::AppState;
use crate::api_error::StartupError;

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
    let url = crate::auth::primitives::auth_url(handle.url(), handle.api_key.expose());
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

/// The message inside a panic payload: the `panic!` string when there
/// is one, or a note that the payload is not a string (a `panic_any`
/// call), which has no displayable message.
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
    // Producers own their own log lines; the hub feeds the UIs only.
    let hub = Arc::new(gateway_progress::ProgressHub::new());
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
    // signal and the gateway discovery file both report the real port.
    let listener = match runtime.block_on(TcpListener::bind(bind)) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = ready.send(Err(StartupError::bind(error)));
            return Ok(());
        }
    };
    // The configured bind may specify port 0; the bound local_addr is the
    // real address the readiness signal must report.
    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => {
            let _ = ready.send(Err(StartupError::bind(error)));
            return Ok(());
        }
    };
    // The shell assembles instantly: the remote routing table over every
    // `[[model]]` and no local runtime (the bind and the
    // publication precede any local work). Local provisioning is the boot
    // command's work, not startup's.
    let boot_profile = profiles.active.clone();
    let gateway = match Gateway::new_with_hub(&config, profiles, hub) {
        Ok(gateway) => gateway,
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    // The gateway discovery file lands before the readiness signal so a spawned
    // gateway is discoverable the moment `spawn` returns; the guard removes
    // it on every exit path below, graceful shutdown included.
    let _gateway_discovery_file = gateway_discovery_file_guard(&config, options, address);
    tracing::info!("gateway serving on {address}");
    let _ = ready.send(Ok(Ready {
        url: format!("http://{address}"),
        api_key: config.server_key(),
        state: gateway.state.clone(),
    }));
    // The boot command lands after the readiness signal: the queue worker
    // loads the selected profile's local models into the live routing table
    // while the gateway is already reachable, and the boot command alone
    // then makes the process's one guarded STT load attempt. The boot
    // selection stays ephemeral, exactly as startup always behaved.
    match boot_selection_notice(&config) {
        Some(BootSelectionNotice::Stale(message)) => tracing::warn!("{message}"),
        Some(BootSelectionNotice::None(message)) => tracing::info!("{message}"),
        None => {}
    }
    gateway.enqueue_boot_load(boot_profile);
    // The cloud provider model sheet loads off the serving path: one
    // bounded background task on the runtime, spawned after boot, with
    // failure contained to the feature. The cache lives in the profile
    // directory beside the other runtime state.
    if let Some(cache_path) = cloud_models_cache_path(options) {
        let cloud_models = gateway.state.cloud_models.clone();
        runtime.spawn(async move {
            cloud_models
                .launch(cache_path, cloud_models_sheet_url())
                .await;
        });
    } else {
        tracing::warn!(
            "no user profile directory found; the cloud provider model sheet is unavailable"
        );
    }
    let result = runtime
        .block_on(gateway.serve(listener, shutdown_on_send(shutdown)))
        .map_err(StartupError::serve);
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
    result
}

/// The sheet download URL: the `PROMPTFORGE_MODELS_SHEET_URL` override
/// when set and non-empty, else the published release artifact.
fn cloud_models_sheet_url() -> String {
    std::env::var(crate::boot::SHEET_URL_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| crate::boot::DEFAULT_SHEET_URL.to_owned())
}

/// The sheet cache path in the profile directory: the run directory's
/// parent, which holds `gateway.toml`, `run/`, and `models/`.
fn cloud_models_cache_path(options: &ServeOptions) -> Option<PathBuf> {
    options
        .run_dir
        .clone()
        .or_else(gateway_api_discovery::default_run_dir)
        .and_then(|run_dir| run_dir.parent().map(Path::to_path_buf))
        .map(|state_dir| state_dir.join(crate::boot::CACHE_FILE_NAME))
}

/// Removes the gateway discovery file on drop when it still belongs to this
/// process, so a graceful shutdown withdraws the gateway from discovery
/// while a replacement's file is spared.
#[derive(Debug)]
struct GatewayDiscoveryFileGuard {
    run_dir: PathBuf,
    pid: u32,
}

impl Drop for GatewayDiscoveryFileGuard {
    fn drop(&mut self) {
        if let Err(error) = gateway_api_discovery::remove_if_mine(&self.run_dir, self.pid) {
            tracing::warn!("could not remove the gateway discovery file: {error}");
        }
    }
}

/// Writes the `gateway.json` gateway discovery file for the just-bound `address`
/// and returns the guard that removes it on drop. A failure is logged and
/// tolerated: the gateway keeps serving, and discovery degrades to a
/// relaunch instead of an attach.
fn gateway_discovery_file_guard(
    config: &Config,
    options: &ServeOptions,
    address: std::net::SocketAddr,
) -> Option<GatewayDiscoveryFileGuard> {
    let Some(run_dir) = options
        .run_dir
        .clone()
        .or_else(gateway_api_discovery::default_run_dir)
    else {
        tracing::warn!("no user profile directory found; no gateway discovery file written");
        return None;
    };
    let now = time::OffsetDateTime::now_utc();
    let file = gateway_api_discovery::GatewayDiscoveryFile {
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
            "could not write the gateway discovery file in {}: {error}; attachers will relaunch instead",
            run_dir.display()
        );
        return None;
    }
    Some(GatewayDiscoveryFileGuard { run_dir, pid })
}
