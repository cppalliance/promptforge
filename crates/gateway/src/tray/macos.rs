//! The macOS tray backend: the `NSApplication` run loop owns the main
//! thread while serving stays on the gateway thread spawned by
//! [`crate::spawn`].
//!
//! Process shape: the gateway ships as a bare executable inside the
//! workshop's .app bundle (`Contents/MacOS/`), so it has no bundle
//! Info.plist of its own and `LSUIElement` is unavailable - the early
//! `setActivationPolicy(.accessory)` call in [`run`] is the mechanism that
//! keeps the daemon out of the Dock, and it runs before any other AppKit
//! initialization. The tray itself is built on the first pass of the run
//! loop (a zero-delay one-shot timer), because status-item construction
//! before the loop runs is the classic source of invisible trays.
//!
//! Menu discipline: muda 0.19.3 does not contain the use-after-free fix
//! for `set_menu` while the menu is displayed
//! (<https://github.com/tauri-apps/muda/issues/328>, fixed by
//! <https://github.com/tauri-apps/muda/pull/361>, merged 2026-07-30 but
//! unreleased), so - exactly as on Windows - every state change mutates
//! the retained `MenuItem` handles in place and `set_menu` is never
//! called after construction.
//! There is no double-click gesture: the menu opens on mouse-down, so
//! Settings is simply the first enabled item. tray-icon delivers the
//! mouse-down `Click` event before it performs the status-item click that
//! opens the menu, which is the one pre-display hook macOS offers for
//! re-probing the Workshop and login states.
//!
//! Launch at Login goes through `SMAppService.mainApp`, which registers
//! the containing bundle's principal executable. Inside the workshop's
//! bundle that principal is the workshop itself, so registration would
//! open the workshop window at every login - the exact annoyance the
//! `--login` design exists to avoid. The store therefore exists only when
//! the gateway is its own bundle's principal executable (a standalone,
//! signed gateway .app); otherwise the menu item is disabled, and an
//! unsigned build's registration failure is reported and rolled back.

use std::cell::RefCell;
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{MainThreadMarker, NSProcessInfo, NSTimer};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use tray_icon::menu::{CheckMenuItem, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::api_error::StartupError;
use crate::runner::{GatewayHandle, ServeOptions, run_headless, spawn};
use crate::tray::logic::{self, TrayPhase};
use crate::tray::menu::{BuiltMenu, MenuBuildError};

/// The status-refresh cadence: the label, tooltip, Workshop enabled bit,
/// and login check all re-read their sources on this timer, never from
/// one-shot events.
const STATUS_INTERVAL: f64 = 5.0;

/// The tray icon's edge length in pixels; the embedded asset is 36x36
/// RGBA, an 18pt template glyph at @2x.
const ICON_SIZE: u32 = 36;

/// The brand glyph as raw RGBA, derived from the workshop's `64x64.png`
/// brand asset (PIL: `Image.open(...).convert("RGBA").resize((36, 36),
/// Image.LANCZOS).tobytes()`; regenerate from
/// `crates/workshop/icons/64x64.png` when the brand changes).
const BRAND_RGBA: &[u8] = include_bytes!("../../assets/tray-icon-template.rgba");

// The asset is exactly one 36x36 RGBA image.
const _: () = assert!(BRAND_RGBA.len() == (ICON_SIZE * ICON_SIZE * 4) as usize);

thread_local! {
    /// The main thread's tray slot, populated by the build timer on the
    /// run loop's first pass and drained by [`run`] after the loop exits.
    /// Every callback - the timers, the tray-icon and menu event handlers -
    /// runs on the main thread inside the run loop's dispatch, and macOS
    /// displays the menu out of process, so there is no in-process modal
    /// loop to reenter; `try_borrow_mut` is belt and braces.
    static TRAY: RefCell<Option<TraySlot>> = const { RefCell::new(None) };
}

/// What the main thread's tray slot holds once the build timer has fired.
enum TraySlot {
    /// The tray is live; the run loop serves it until Quit. Boxed: the
    /// armed state dwarfs the headless handle.
    Armed(Box<Tray>),
    /// Tray construction failed; the handle returns to the headless loop.
    Headless(GatewayHandle),
}

/// Sets the accessory activation policy and runs the tray on the main
/// thread until Quit. A tray that cannot start degrades to the headless
/// Ctrl-C loop: the gateway is already serving and the tray is its face,
/// not its life support.
pub(super) fn run(options: &ServeOptions) -> Result<(), StartupError> {
    let Some(mtm) = MainThreadMarker::new() else {
        // `run_with_tray` is called from `main`; a non-main-thread caller
        // gets the headless loop rather than a panic.
        tracing::error!("the tray backend must run on the main thread; running headless");
        return crate::run(options);
    };
    let app = NSApplication::sharedApplication(mtm);
    // The bare executable has no Info.plist, so this call is the only
    // mechanism that keeps the gateway out of the Dock; it must run before
    // any other AppKit object is created.
    if !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
        tracing::warn!(
            "could not set the accessory activation policy; the gateway may appear in the Dock"
        );
    }
    let handle = spawn(options)?;
    install_event_handlers();
    // Build the tray on the run loop's first pass: a timer interval of
    // zero clamps to 0.1 ms and fires once the loop is running.
    let handle = RefCell::new(Some(handle));
    let build: RcBlock<dyn Fn(NonNull<NSTimer>)> = RcBlock::new(move |_timer: NonNull<NSTimer>| {
        let handle = handle
            .borrow_mut()
            .take()
            .unwrap_or_else(|| unreachable!("the build timer fires exactly once"));
        let slot = match Tray::build(&handle) {
            Ok(tray) => TraySlot::Armed(Box::new(tray.arm(handle))),
            Err(error) => {
                tracing::error!("the system tray is unavailable: {error}; running headless");
                TraySlot::Headless(handle)
            }
        };
        let headless = matches!(slot, TraySlot::Headless(_));
        TRAY.with(|cell| *cell.borrow_mut() = Some(slot));
        if headless {
            // No tray to quit through; leave the run loop so the headless
            // Ctrl-C loop below takes over.
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).stop(None);
            }
        }
    });
    // SAFETY: the block is scheduled on the current (main) run loop and
    // only ever fires on this thread, so its captures never cross threads.
    let build_timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &build) };
    app.run();
    build_timer.invalidate();
    let slot = TRAY.with(|cell| cell.borrow_mut().take());
    match slot {
        Some(TraySlot::Armed(tray)) => tray.teardown(),
        Some(TraySlot::Headless(handle)) => run_headless(handle),
        None => {
            // Unreachable in practice: the build timer fires on the first
            // loop pass, before any event that could stop the loop. The
            // gateway thread's connection-file guard is lost with the
            // process, which stale-file detection covers on next launch.
            tracing::error!("the run loop exited before the tray was built");
            Ok(())
        }
    }
}

/// Why the tray could not start; [`run`] falls back to headless serving.
#[derive(Debug, thiserror::Error)]
enum TrayError {
    /// The tray was not built on the main thread.
    #[error("the tray must be built on the main thread")]
    NotMainThread,
    /// The embedded icon asset did not decode.
    #[error("decode the tray icon")]
    Icon(#[source] tray_icon::BadIcon),
    /// The menu could not be assembled.
    #[error(transparent)]
    Menu(#[from] MenuBuildError),
    /// The tray icon could not be registered with the system.
    #[error("register the tray icon")]
    Register(#[source] tray_icon::Error),
}

/// An event delivered by a tray-icon or muda callback on the main thread.
enum TrayEvent {
    /// A menu item was activated.
    Menu(MenuId),
    /// A mouse button went down on the status item: the menu is about to
    /// open, so re-probe the states it is about to show.
    IconDown,
}

/// The tray's whole state, living in the main thread's [`TRAY`] slot.
struct Tray {
    /// The shared application, for `stop` on Quit.
    app: Retained<NSApplication>,
    /// The status-refresh timer; retained so teardown can invalidate it.
    tick_timer: Retained<NSTimer>,
    /// The tray icon; `None` after teardown takes it. Never cloned: the
    /// icon is reference-counted and a surviving clone leaks a ghost
    /// status item.
    icon: Option<TrayIcon>,
    /// The current icon phase. The template glyph's tint belongs to the
    /// system, so the phase travels in the label and tooltip only.
    phase: TrayPhase,
    /// The last rendered status text, so an unchanged tick skips the
    /// status-item update.
    label: String,
    /// The disabled status line at the top of the menu.
    status_item: MenuItem,
    /// Launches the workshop shell.
    workshop_item: MenuItem,
    /// Opens the config SPA.
    settings_item: MenuItem,
    /// The launch-at-login check item.
    login_item: CheckMenuItem,
    /// Quits the gateway.
    quit_item: MenuItem,
    /// The launch-at-login store; `None` when SMAppService cannot name
    /// this process (macOS older than 13, or a bare executable inside
    /// another app's bundle), which is also the menu item's disabled
    /// state.
    login: Option<LoginService>,
    /// The running gateway: the status source and the shutdown switch.
    /// `None` only while teardown takes it.
    handle: Option<GatewayHandle>,
    /// The one-time browser handoff URL for the Settings item.
    auth_url: String,
    /// The workshop exe beside ours, when present.
    workshop_exe: Option<PathBuf>,
}

impl Tray {
    /// Everything fallible, before the tray is armed: the icon, the menu,
    /// the login store, the status-item registration, and the refresh
    /// timer. The gateway handle is only read, so a failure can hand it
    /// back.
    fn build(handle: &GatewayHandle) -> Result<Tray, TrayError> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Err(TrayError::NotMainThread);
        };
        let app = NSApplication::sharedApplication(mtm);
        let glyph = logic::macos::template_glyph(BRAND_RGBA);
        let glyph = Icon::from_rgba(glyph, ICON_SIZE, ICON_SIZE).map_err(TrayError::Icon)?;
        let auth_url = crate::handoff::auth_url(handle.url(), handle.tray_key());
        let workshop_exe = probe_workshop();
        let login = LoginService::new();
        let login_checked = login
            .as_ref()
            .is_some_and(|service| logic::launch_at_login(service));
        let label = logic::status_label(TrayPhase::Starting, None, 0, 0.0);
        let spec = logic::menu_spec(
            &label,
            workshop_exe.is_some(),
            login.is_some(),
            login_checked,
        );
        let menu = BuiltMenu::from_spec(&spec)?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu.menu))
            .with_tooltip(&label)
            // The builder applies the icon and its template flag in one
            // status-item update; a visible icon must never see the split
            // `set_icon` + `set_icon_as_template` sequence, which renders
            // twice and visibly flickers. Any future icon swap goes
            // through `set_icon_with_as_template` for the same reason.
            .with_icon(glyph)
            .with_icon_as_template(true)
            .build()
            .map_err(TrayError::Register)?;
        let tick: RcBlock<dyn Fn(NonNull<NSTimer>)> =
            RcBlock::new(move |_timer: NonNull<NSTimer>| dispatch_tick());
        // SAFETY: the block is scheduled on the current (main) run loop
        // and only ever fires on this thread, so it never crosses threads.
        let tick_timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(STATUS_INTERVAL, true, &tick)
        };
        Ok(Tray {
            app,
            tick_timer,
            icon: Some(icon),
            phase: TrayPhase::Starting,
            label,
            status_item: menu.status,
            workshop_item: menu.workshop,
            settings_item: menu.settings,
            login_item: menu.login,
            quit_item: menu.quit,
            login,
            handle: None,
            auth_url,
            workshop_exe,
        })
    }

    /// Arms the tray: stores the gateway handle and runs the first status
    /// refresh. Infallible: every fallible step happened in
    /// [`Tray::build`], so a failure there can hand the handle back to the
    /// headless fallback.
    fn arm(mut self, handle: GatewayHandle) -> Tray {
        self.handle = Some(handle);
        tick(&mut self);
        self
    }

    /// Tears down after the run loop exits: the tick timer first (it
    /// retains the block that touches the tray slot), then the tray icon
    /// (a surviving reference leaks a ghost status item), then the
    /// gateway's graceful shutdown - never `process::exit` ahead of
    /// destructors, so the connection-file guard still runs.
    fn teardown(mut self) -> Result<(), StartupError> {
        self.tick_timer.invalidate();
        drop(self.icon.take());
        let handle = self.handle.take();
        drop(self);
        match handle {
            Some(handle) => handle.shutdown(),
            None => Ok(()),
        }
    }
}

/// Installs the tray-icon and menu callbacks. On macOS both fire on the
/// main thread inside the run loop's dispatch (tray-icon sends its mouse
/// events from the status item's NSView, muda from the menu item's
/// action), so the handlers act directly on the thread-local tray.
fn install_event_handlers() {
    TrayIconEvent::set_event_handler(Some(|event| {
        // tray-icon delivers the mouse-down Click before it performs the
        // status-item click that opens the menu, so this handler is the
        // pre-display refresh hook.
        if matches!(
            event,
            TrayIconEvent::Click {
                button_state: MouseButtonState::Down,
                ..
            }
        ) {
            dispatch_event(TrayEvent::IconDown);
        }
    }));
    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        dispatch_event(TrayEvent::Menu(event.id));
    }));
}

/// Routes one event to the armed tray. The handlers are installed
/// process-wide; before the build timer arms the slot, events drop.
fn dispatch_event(event: TrayEvent) {
    TRAY.with(|cell| {
        let Ok(mut cell) = cell.try_borrow_mut() else {
            tracing::debug!("dropping a tray event during a reentrant dispatch");
            return;
        };
        if let Some(TraySlot::Armed(tray)) = cell.as_mut() {
            act(tray, event);
        }
    });
}

/// Runs the status refresh on the armed tray, if any.
fn dispatch_tick() {
    TRAY.with(|cell| {
        let Ok(mut cell) = cell.try_borrow_mut() else {
            // The timer is memoryless, so a skipped tick loses nothing.
            tracing::debug!("skipping a reentrant tray tick");
            return;
        };
        if let Some(TraySlot::Armed(tray)) = cell.as_mut() {
            tick(tray);
        }
    });
}

/// The status refresh: recomputes the phase from the gateway thread's
/// liveness and the label from the gateway's in-process state, and keeps
/// the Workshop enabled bit and the login check honest. Nothing here
/// blocks: a profile switch holding the live-state lock simply skips the
/// label update for one tick.
fn tick(tray: &mut Tray) {
    let Some(handle) = tray.handle.as_ref() else {
        return;
    };
    let poll = if handle.is_serving() {
        logic::Poll::Serving
    } else {
        logic::Poll::Stopped
    };
    if poll == logic::Poll::Stopped && handle.tray_state().shutdown.is_fired() {
        // A requested shutdown (POST /shutdown, the shell's
        // Quit-everything): the gateway's work is done, so the process
        // follows it through the normal teardown. Without this the run
        // loop would pump a dead gateway forever, showing a false error.
        request_quit(tray);
        return;
    }
    tray.phase = logic::next_phase(poll);
    let command = handle.tray_state().commands.active_command();
    if let Some((models, vram_gb)) = handle.tray_state().tray_model_status() {
        let label = logic::status_label(
            tray.phase,
            command
                .as_ref()
                .map(|active| (active.name.as_str(), active.progress)),
            models,
            vram_gb,
        );
        if label != tray.label {
            tray.status_item.set_text(&label);
            if let Some(icon) = tray.icon.as_ref()
                && let Err(error) = icon.set_tooltip(Some(&label))
            {
                tracing::debug!("could not update the tray tooltip: {error}");
            }
            tray.label = label;
        }
    }
    refresh_menu_state(tray);
}

/// One event, handled on the main thread.
fn act(tray: &mut Tray, event: TrayEvent) {
    match event {
        TrayEvent::IconDown => refresh_menu_state(tray),
        TrayEvent::Menu(id) => {
            if id == *tray.quit_item.id() {
                request_quit(tray);
            } else if id == *tray.workshop_item.id() {
                launch_workshop(tray);
            } else if id == *tray.settings_item.id() {
                open_settings(tray);
            } else if id == *tray.login_item.id() {
                toggle_login(tray);
            }
        }
    }
}

/// Re-reads the states the menu shows - the Workshop sibling probe and
/// the login check - so a displayed menu never shows a stale enabled bit
/// or check mark. Called on the pre-display mouse-down and on every tick.
fn refresh_menu_state(tray: &mut Tray) {
    tray.workshop_exe = probe_workshop();
    tray.workshop_item.set_enabled(tray.workshop_exe.is_some());
    if let Some(login) = tray.login.as_ref() {
        tray.login_item.set_checked(logic::launch_at_login(login));
    }
}

/// Opens the config SPA in the default browser through the one-time
/// handoff URL, so the bearer key never sits in browser history.
fn open_settings(tray: &Tray) {
    if let Err(error) = open::that(&tray.auth_url) {
        tracing::warn!("could not open the settings page: {error}");
    }
}

/// Launches the workshop shell through Launch Services: `/usr/bin/open`
/// with the containing .app bundle is sandbox-immune (NSWorkspace
/// argument passing is not) and resolves the bundle's principal
/// executable. The workshop attaches to this gateway through the
/// connection file and outlives it. An unbundled dev run spawns the
/// sibling executable directly.
fn launch_workshop(tray: &Tray) {
    let Some(exe) = tray.workshop_exe.as_ref() else {
        return;
    };
    let mut command = if let Some(bundle) = logic::macos::app_bundle(exe) {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg(bundle);
        command
    } else {
        // The unbundled dev fallback detaches the way the shell's own
        // gateway spawn does (crates/workshop/src/gateway.rs): its own
        // process group, so a terminal Ctrl-C on the gateway does not
        // SIGINT the workshop.
        let mut command = std::process::Command::new(exe);
        command.process_group(0);
        command
    };
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match child {
        Ok(mut child) => {
            // `open` exits as soon as Launch Services takes over; reap it
            // off the run loop so repeated clicks cannot accumulate
            // zombie children.
            let reaper = std::thread::Builder::new()
                .name("open-reaper".to_string())
                .spawn(move || {
                    // The reap is the whole job; a failed wait means the
                    // child is already gone and there is nothing to report.
                    let _ = child.wait();
                });
            if let Err(error) = reaper {
                // The child is reaped at process exit instead.
                tracing::debug!("could not spawn the open reaper: {error}");
            }
        }
        Err(error) => tracing::warn!("could not launch {}: {error}", exe.display()),
    }
}

/// Toggles the launch-at-login registration and reflects the result in
/// the check item. muda toggles the check on click, before the event
/// arrives; a failed write restores the OS state immediately rather than
/// waiting for the next tick.
fn toggle_login(tray: &mut Tray) {
    let Tray {
        login, login_item, ..
    } = tray;
    let Some(store) = login.as_mut() else {
        // The item is disabled without a store, so no click arrives.
        return;
    };
    let enable = !logic::launch_at_login(store);
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            tracing::warn!("could not locate the gateway executable: {error}");
            return;
        }
    };
    match logic::set_launch_at_login(store, &logic::run_key_command(&exe), enable) {
        Ok(enabled) => login_item.set_checked(enabled),
        Err(error) => {
            tracing::warn!("could not update the login registration: {error}");
            login_item.set_checked(logic::launch_at_login(store));
        }
    }
}

/// Ends the run loop. `stop` is observed once the current event finishes
/// dispatching, and both call sites - the Quit menu action and the tick
/// timer - run inside event processing, so the loop exits promptly.
fn request_quit(tray: &Tray) {
    tray.app.stop(None);
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

/// The launch-at-login store backed by `SMAppService.mainApp`, the modern
/// replacement for LaunchAgents plist installs (which surface badly in
/// System Settings and can trigger TCC prompts).
struct LoginService {
    service: Retained<SMAppService>,
}

impl LoginService {
    /// The store exists only when SMAppService can name this process:
    /// macOS 13 or later (the class arrived in 13, and messaging an
    /// absent class panics inside the objc2 class lookup), and the
    /// gateway running as its bundle's principal executable. Inside the
    /// workshop's .app - the shipped shape - the principal is the
    /// workshop, so registration would open the workshop window at every
    /// login, and the item stays disabled instead. Registration is
    /// additionally meaningful only for signed builds: an unsigned bundle
    /// fails at register time with kSMErrorInvalidSignature, which the
    /// toggle reports and rolls back.
    fn new() -> Option<LoginService> {
        let version = NSProcessInfo::processInfo().operatingSystemVersion();
        let Ok(major) = u64::try_from(version.majorVersion) else {
            return None;
        };
        if !logic::macos::login_service_supported(major) {
            tracing::warn!("SMAppService requires macOS 13; Launch at Login is unavailable");
            return None;
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(error) => {
                tracing::warn!("could not locate the gateway executable: {error}");
                return None;
            }
        };
        let bundle = logic::macos::app_bundle(&exe)?;
        if !logic::macos::gateway_is_bundle_principal(&bundle, &exe) {
            tracing::info!(
                "the gateway is not its bundle's principal executable; \
                 Launch at Login is unavailable"
            );
            return None;
        }
        // SAFETY: main thread, and the SMAppService class exists because
        // the OS version was checked above.
        let service = unsafe { SMAppService::mainAppService() };
        Some(LoginService { service })
    }
}

impl logic::RunKeyStore for LoginService {
    fn read(&self) -> Option<String> {
        // SAFETY: main thread; a pure query on the live service object.
        let status = unsafe { self.service.status() };
        logic::macos::login_registered(status.into()).then(|| "SMAppService.mainApp".to_owned())
    }

    fn write(&mut self, _command: &str) -> std::io::Result<()> {
        // The command line is the Windows Run-key shape; SMAppService
        // launches the bundle's principal executable with no arguments.
        // SAFETY: main thread; registration of the main-app login item.
        unsafe { self.service.registerAndReturnError() }
            .map_err(|error| std::io::Error::other(format!("{error:?}")))
    }

    fn delete(&mut self) -> std::io::Result<()> {
        // SAFETY: main thread; unregistration of the main-app login item.
        unsafe { self.service.unregisterAndReturnError() }
            .map_err(|error| std::io::Error::other(format!("{error:?}")))
    }
}

impl From<SMAppServiceStatus> for logic::macos::LoginServiceStatus {
    fn from(status: SMAppServiceStatus) -> Self {
        if status == SMAppServiceStatus::Enabled {
            Self::Enabled
        } else if status == SMAppServiceStatus::RequiresApproval {
            Self::RequiresApproval
        } else if status == SMAppServiceStatus::NotRegistered {
            Self::NotRegistered
        } else {
            Self::NotFound
        }
    }
}
