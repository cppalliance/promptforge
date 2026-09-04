//! The Linux tray backend: a pure StatusNotifierItem over the session
//! D-Bus via ksni - no GTK, no libappindicator, no GTK main thread to
//! appease.
//!
//! Runtime shape: serving stays on the gateway thread's runtime spawned by
//! [`crate::spawn`], and the tray drives ksni from its own current-thread
//! runtime on the main thread - the house one-runtime-per-thread pattern,
//! same as the Ctrl-C handler. ksni's async-io build runs the D-Bus
//! service on its own executor thread, so tray callbacks never depend on
//! which runtime polls them (ksni's default `tokio` feature couples the
//! service to a borrowed tokio context; the async-io feature exists to
//! avoid that, per the ksni crate docs).
//!
//! There are no icon click events: SNI delivers `Activate` at the
//! visualization's discretion and libappindicator-semantics desktops never
//! deliver it, so the menu is the only path. `MENU_ON_ACTIVATE` makes even
//! the activation gesture open the menu where a visualization offers one.
//!
//! The no-watcher fallback: stock GNOME runs no StatusNotifierWatcher
//! without the AppIndicator extension, so the tray is spawned with
//! `assume_sni_available(true)` - a missing watcher is routed to
//! `watcher_offline` instead of failing the spawn, the daemon keeps
//! serving, and ksni's service loop re-registers automatically when a
//! watcher appears later (it tracks `NameOwnerChanged` on the watcher's
//! well-known name). On the first run without a watcher, one
//! `org.freedesktop.Notifications` notification names the Settings URL; a
//! sentinel file keeps it from ever repeating.
//!
//! Menu state flows through [`ksni::Handle::update`]: the tray object is
//! the source of truth, and ksni re-reads and diffs it on every update, on
//! every click, and - because `menu_about_to_show` is implemented - before
//! every menu display, which is the pre-display re-probe hook muda lacks.

use std::path::PathBuf;
use std::time::Duration;

use ksni::TrayMethods as _;
use ksni::menu::{CheckmarkItem, StandardItem};

use crate::api_error::StartupError;
use crate::handoff::auth_url;
use crate::runner::{GatewayHandle, ServeOptions, run_headless, spawn};
use crate::tray::logic::{self, MenuItemSpec, TrayPhase};

/// The status-refresh cadence: the label, title, and icon phase re-read
/// their sources on this timer, never from one-shot events.
const STATUS_INTERVAL: Duration = Duration::from_secs(5);

/// The tray icon's edge length in pixels; the embedded asset is 32x32
/// RGBA, the same brand asset the Windows backend draws on.
const ICON_SIZE: i32 = 32;

/// The brand icon as raw RGBA (regenerate from
/// `crates/workshop/icons/32x32.png` when the brand changes).
const BRAND_RGBA: &[u8] = include_bytes!("../../assets/tray-icon.rgba");

// The asset is exactly one 32x32 RGBA image.
const _: () = assert!(BRAND_RGBA.len() == 32 * 32 * 4);

/// Spawns the gateway, then runs the tray until Quit. A tray that cannot
/// start degrades to the headless Ctrl-C loop: the gateway is already
/// serving and the tray is its face, not its life support.
pub(super) fn run(options: &ServeOptions) -> Result<(), StartupError> {
    let handle = spawn(options)?;
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!("could not build the tray runtime: {error}; running headless");
            return run_headless(handle);
        }
    };
    match runtime.block_on(tray_loop(&handle)) {
        Outcome::Quit => handle.shutdown(),
        Outcome::Unavailable => run_headless(handle),
        Outcome::ServiceGone => {
            tracing::error!("the tray service ended; running headless");
            run_headless(handle)
        }
    }
}

/// How the tray loop ended.
#[derive(Debug)]
enum Outcome {
    /// The user quit, or a requested shutdown fired: tear the gateway down.
    Quit,
    /// The tray never started; the caller falls back to headless serving.
    Unavailable,
    /// The tray service ended under us (the session bus went away); the
    /// caller keeps serving headless.
    ServiceGone,
}

/// What one refresh tick concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tick {
    /// Serving continues.
    Continue,
    /// A requested shutdown fired: quit.
    Quit,
    /// The tray service is gone.
    ServiceGone,
}

/// The tray's whole lifecycle on the main thread's runtime: start, refresh
/// on the timer until Quit or the service's end, then deregister.
async fn tray_loop(handle: &GatewayHandle) -> Outcome {
    let (quit_tx, quit_rx) = tokio::sync::mpsc::unbounded_channel();
    let Some(tray) = start_tray(handle, quit_tx).await else {
        return Outcome::Unavailable;
    };
    let outcome = serve_loop(&tray, quit_rx, handle).await;
    // Deregister the item and close the bus connection before the gateway
    // teardown; on a dead service the shutdown is a no-op.
    tray.shutdown().await;
    outcome
}

/// The refresh loop: the status timer and the Quit signal, whichever
/// moves first.
async fn serve_loop(
    tray: &ksni::Handle<SniTray>,
    mut quit: tokio::sync::mpsc::UnboundedReceiver<()>,
    handle: &GatewayHandle,
) -> Outcome {
    let mut interval = tokio::time::interval(STATUS_INTERVAL);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                match tick(tray, handle).await {
                    Tick::Continue => {}
                    Tick::Quit => return Outcome::Quit,
                    Tick::ServiceGone => return Outcome::ServiceGone,
                }
            }
            quit = quit.recv() => {
                return match quit {
                    Some(()) => Outcome::Quit,
                    // The sender dropped with the tray object: the service
                    // is gone.
                    None => Outcome::ServiceGone,
                };
            }
        }
    }
}

/// The status refresh: recomputes the phase from the gateway thread's
/// liveness and the label from the gateway's in-process state. Nothing
/// here blocks: a profile switch holding the live-state lock skips the
/// label for one tick. ksni diffs the tray against what it last emitted,
/// so an unchanged update costs no D-Bus traffic.
async fn tick(tray: &ksni::Handle<SniTray>, handle: &GatewayHandle) -> Tick {
    let poll = if handle.is_serving() {
        logic::Poll::Serving
    } else {
        logic::Poll::Stopped
    };
    if poll == logic::Poll::Stopped && handle.tray_state().shutdown.is_fired() {
        // A requested shutdown (POST /shutdown, the shell's
        // Quit-everything): the gateway's work is done, so the process
        // follows it through the normal teardown rather than showing a
        // false error forever.
        return Tick::Quit;
    }
    let phase = logic::next_phase(poll);
    let label = handle
        .tray_state()
        .tray_model_status()
        .map(|(models, vram_gb)| logic::status_label(phase, models, vram_gb));
    let updated = tray
        .update(|tray| {
            tray.phase = phase;
            if let Some(label) = label {
                tray.label = label;
            }
        })
        .await;
    if updated.is_none() {
        return Tick::ServiceGone;
    }
    Tick::Continue
}

/// Everything fallible, before the refresh loop starts: the session-bus
/// probe, the first-run notification, and the SNI registration. The
/// gateway handle is only read, so a failure can hand it back to the
/// headless fallback.
async fn start_tray(
    handle: &GatewayHandle,
    quit: tokio::sync::mpsc::UnboundedSender<()>,
) -> Option<ksni::Handle<SniTray>> {
    let settings_url = auth_url(handle.url(), handle.tray_key());
    let bus = match zbus::Connection::session().await {
        Ok(bus) => Some(bus),
        Err(error) => {
            // A headless server has no session bus at all.
            tracing::warn!("no session D-Bus: {error}; the tray is unavailable");
            None
        }
    };
    if let Some(bus) = &bus
        && !watcher_present(bus).await
    {
        tracing::info!(
            "no StatusNotifierWatcher on the session bus; the tray appears when one starts"
        );
        notify_first_run(bus, &settings_url).await;
    }
    let tray = SniTray::new(handle, quit);
    // assume_sni_available: a missing watcher is not an error. The service
    // stays up and re-registers when a watcher appears, which is the whole
    // GNOME story; a genuine failure (no session bus) still falls back.
    match tray.assume_sni_available(true).spawn().await {
        Ok(handle) => Some(handle),
        Err(error) => {
            tracing::error!("the system tray is unavailable: {error}; running headless");
            None
        }
    }
}

/// Whether a StatusNotifierWatcher owns its well-known name on the
/// session bus. A probe failure reads as absent: registration below
/// reports the real error when the bus itself is the problem.
async fn watcher_present(bus: &zbus::Connection) -> bool {
    let name = match zbus::names::BusName::from_static_str(logic::linux::WATCHER_NAME) {
        Ok(name) => name,
        Err(error) => {
            // The constant is a valid bus name by construction and pinned
            // by test; a rejection means zbus changed its contract.
            tracing::error!("the watcher bus name was rejected: {error}");
            return false;
        }
    };
    let proxy = match zbus::fdo::DBusProxy::new(bus).await {
        Ok(proxy) => proxy,
        Err(error) => {
            tracing::warn!("could not probe the session bus: {error}");
            return false;
        }
    };
    match proxy.name_has_owner(name).await {
        Ok(present) => present,
        Err(error) => {
            tracing::warn!("could not probe for the StatusNotifierWatcher: {error}");
            false
        }
    }
}

/// Posts the no-watcher notification naming the Settings URL, once per
/// install: the sentinel is written only after a successful post, so a
/// failed notification (no notification daemon) is retried next run.
async fn notify_first_run(bus: &zbus::Connection, settings_url: &str) {
    let Some(home) = std::env::home_dir() else {
        return;
    };
    let marker = logic::linux::notification_marker(&home);
    if marker.exists() {
        return;
    }
    if let Err(error) = post_notification(bus, settings_url).await {
        tracing::warn!("could not post the no-tray notification: {error}");
        return;
    }
    if let Some(dir) = marker.parent()
        && let Err(error) = std::fs::create_dir_all(dir)
    {
        tracing::warn!("could not create {}: {error}", dir.display());
        return;
    }
    if let Err(error) = std::fs::write(&marker, "") {
        tracing::warn!("could not write the notification sentinel: {error}");
    }
}

/// One `org.freedesktop.Notifications` `Notify` call: no icon, no actions,
/// the daemon-default timeout. The reply (the notification id) is unused.
async fn post_notification(bus: &zbus::Connection, settings_url: &str) -> Result<(), zbus::Error> {
    let hints: std::collections::HashMap<&str, zbus::zvariant::Value> =
        std::collections::HashMap::new();
    bus.call_method(
        Some(logic::linux::NOTIFICATIONS_NAME),
        logic::linux::NOTIFICATIONS_PATH,
        Some(logic::linux::NOTIFICATIONS_NAME),
        "Notify",
        &(
            "PromptForge Gateway",
            0u32,
            "",
            "PromptForge Gateway",
            logic::linux::notification_body(settings_url),
            Vec::<String>::new(),
            hints,
            -1i32,
        ),
    )
    .await?;
    Ok(())
}

/// The three phase icons, precomputed once from the embedded asset.
struct PhaseIcons {
    starting: ksni::Icon,
    running: ksni::Icon,
    error: ksni::Icon,
}

impl PhaseIcons {
    /// Converts the embedded RGBA asset and its grayed and error variants
    /// to ksni's ARGB32.
    fn load() -> PhaseIcons {
        let icon = |rgba: &[u8]| ksni::Icon {
            width: ICON_SIZE,
            height: ICON_SIZE,
            data: logic::linux::to_argb(rgba),
        };
        PhaseIcons {
            starting: icon(&logic::grayed(BRAND_RGBA)),
            running: icon(BRAND_RGBA),
            error: icon(&logic::error_tint(BRAND_RGBA)),
        }
    }

    /// The icon for one phase.
    fn for_phase(&self, phase: TrayPhase) -> &ksni::Icon {
        match phase {
            TrayPhase::Starting => &self.starting,
            TrayPhase::Running => &self.running,
            TrayPhase::Error => &self.error,
        }
    }
}

/// The tray's state, owned by the ksni service: every callback and every
/// update closure runs on ksni's executor thread, serialized by the
/// service lock, so the actions run where the state lives.
struct SniTray {
    /// The status line: the menu's disabled top label, the title, and the
    /// tooltip text.
    label: String,
    /// The current icon phase.
    phase: TrayPhase,
    /// The per-phase icons, precomputed from the embedded asset.
    icons: PhaseIcons,
    /// The one-time browser handoff URL for the Settings item.
    auth_url: String,
    /// The workshop exe beside ours, when present.
    workshop_exe: Option<PathBuf>,
    /// The XDG autostart store; `None` when the home directory is
    /// unresolvable, which is also the menu item's disabled state.
    login: Option<XdgAutostart>,
    /// The check item's state, read from the entry's presence.
    login_checked: bool,
    /// Signals the main loop to quit.
    quit: tokio::sync::mpsc::UnboundedSender<()>,
}

impl SniTray {
    /// Builds the tray's initial state: Starting phase, the sibling and
    /// login probes read once (the pre-display hook keeps them honest).
    fn new(handle: &GatewayHandle, quit: tokio::sync::mpsc::UnboundedSender<()>) -> SniTray {
        let login = XdgAutostart::new();
        let login_checked = login
            .as_ref()
            .is_some_and(|store| logic::launch_at_login(store));
        SniTray {
            label: logic::status_label(TrayPhase::Starting, 0, 0.0),
            phase: TrayPhase::Starting,
            icons: PhaseIcons::load(),
            auth_url: auth_url(handle.url(), handle.tray_key()),
            workshop_exe: probe_workshop(),
            login,
            login_checked,
            quit,
        }
    }

    /// Opens the config SPA in the default browser through the one-time
    /// handoff URL, so the bearer key never sits in browser history.
    fn open_settings(&self) {
        if let Err(error) = open::that(&self.auth_url) {
            tracing::warn!("could not open the settings page: {error}");
        }
    }

    /// Launches the workshop shell, detached: its own process group, so a
    /// terminal Ctrl-C on the gateway does not SIGINT the workshop. It
    /// attaches to this gateway through the connection file and outlives
    /// it.
    fn launch_workshop(&self) {
        use std::os::unix::process::CommandExt as _;

        let Some(exe) = self.workshop_exe.as_ref() else {
            return;
        };
        let mut command = std::process::Command::new(exe);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        if let Err(error) = command.spawn() {
            tracing::warn!("could not launch {}: {error}", exe.display());
        }
    }

    /// Toggles the XDG autostart entry and reflects the result in the
    /// check item; a failed write leaves the state reading from the file.
    fn toggle_login(&mut self) {
        let Some(store) = self.login.as_mut() else {
            // The item is disabled without a store, so no click arrives.
            return;
        };
        let enable = !logic::launch_at_login(store);
        let exe = match std::env::current_exe() {
            Ok(exe) => {
                // The Exec line names the canonicalized exe: current_exe is
                // the resolved /proc/self/exe on Linux, and canonicalize
                // covers symlinked launchers.
                std::fs::canonicalize(&exe).unwrap_or(exe)
            }
            Err(error) => {
                tracing::warn!("could not locate the gateway executable: {error}");
                return;
            }
        };
        match logic::set_launch_at_login(store, &logic::linux::exec_command(&exe), enable) {
            Ok(enabled) => self.login_checked = enabled,
            Err(error) => {
                tracing::warn!("could not update the autostart entry: {error}");
                self.login_checked = logic::launch_at_login(store);
            }
        }
    }

    /// Signals the main loop to quit.
    fn request_quit(&self) {
        // A failed send means the main loop already ended, which is the
        // quit state being requested.
        let _ = self.quit.send(());
    }
}

impl ksni::Tray for SniTray {
    // The menu is the only path: SNI delivers icon click events at the
    // visualization's discretion, so activation opens the menu instead of
    // an action.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "promptforge-gateway".to_owned()
    }

    fn title(&self) -> String {
        self.label.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icons.for_phase(self.phase).clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.label.clone(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        logic::menu_spec(
            &self.label,
            self.workshop_exe.is_some(),
            self.login.is_some(),
            self.login_checked,
        )
        .into_iter()
        .map(|item| match item {
            MenuItemSpec::Status(text) => StandardItem {
                label: text,
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItemSpec::Workshop { enabled } => StandardItem {
                label: "Workshop".to_owned(),
                enabled,
                activate: Box::new(|tray: &mut Self| tray.launch_workshop()),
                ..Default::default()
            }
            .into(),
            MenuItemSpec::Settings => StandardItem {
                label: "Settings".to_owned(),
                activate: Box::new(|tray: &mut Self| tray.open_settings()),
                ..Default::default()
            }
            .into(),
            MenuItemSpec::Separator => ksni::MenuItem::Separator,
            MenuItemSpec::LaunchAtLogin { enabled, checked } => CheckmarkItem {
                label: "Launch at Login".to_owned(),
                enabled,
                checked,
                activate: Box::new(|tray: &mut Self| tray.toggle_login()),
                ..Default::default()
            }
            .into(),
            MenuItemSpec::Quit => StandardItem {
                label: "Quit".to_owned(),
                activate: Box::new(|tray: &mut Self| tray.request_quit()),
                ..Default::default()
            }
            .into(),
        })
        .collect()
    }

    fn menu_about_to_show(&mut self) {
        // The pre-display refresh: re-probe the states the menu is about
        // to show, so it never displays a stale enabled bit or check mark.
        self.workshop_exe = probe_workshop();
        if let Some(store) = self.login.as_ref() {
            self.login_checked = logic::launch_at_login(store);
        }
    }

    fn watcher_online(&self) {
        tracing::info!("a StatusNotifierWatcher appeared; the tray is live");
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::info!(
            "the StatusNotifierWatcher is offline ({reason:?}); the tray re-registers when one appears"
        );
        // Stay alive: returning false would shut the service down and lose
        // the automatic re-registration.
        true
    }
}

/// The launch-at-login store backed by the XDG autostart entry. The entry
/// is written only by this toggle - never at boot, never by the
/// installer - so a user-deleted entry stays deleted, and the check state
/// reads the file's presence, never local config.
struct XdgAutostart {
    /// The entry's path, resolved from the environment at construction.
    path: PathBuf,
}

impl XdgAutostart {
    /// Resolves the entry path; `None` when the home directory is
    /// unresolvable.
    fn new() -> Option<XdgAutostart> {
        let home = std::env::home_dir()?;
        Some(XdgAutostart {
            path: logic::linux::autostart_path(
                std::env::var_os("XDG_CONFIG_HOME").as_deref(),
                &home,
            ),
        })
    }
}

impl logic::RunKeyStore for XdgAutostart {
    fn read(&self) -> Option<String> {
        // The state is the file's presence alone.
        self.path.exists().then(String::new)
    }

    fn write(&mut self, command: &str) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, logic::linux::desktop_entry(command))
    }

    fn delete(&mut self) -> std::io::Result<()> {
        // Deleting an absent entry succeeds: delete is idempotent.
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// The workshop exe beside this process's exe, when the installer laid
/// one there.
fn probe_workshop() -> Option<PathBuf> {
    match std::env::current_exe() {
        Ok(exe) => logic::workshop_sibling(&exe),
        Err(error) => {
            tracing::warn!("could not locate the gateway executable: {error}");
            None
        }
    }
}
