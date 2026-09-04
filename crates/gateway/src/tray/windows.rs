//! The Windows tray backend: a hidden message-only window runs the main
//! thread's win32 message loop while serving stays on the gateway thread.
//!
//! tray-icon delivers its callbacks on this thread from inside the loop's
//! dispatch, and muda's menu items are `!Send` (they hold
//! `Rc<RefCell<_>>` state), so a callback cannot touch menu state ahead of
//! the crate's automatic menu show. The backend therefore disables both
//! auto-shows and opens the menu itself from the loop after re-probing the
//! Workshop sibling - the ordering the tray idiom needs - and the
//! callbacks only forward a [`TrayEvent`] into the loop with a
//! `PostMessageW` wake, never launching processes, opening browsers, or
//! touching the registry.

use std::cell::Cell;
use std::os::windows::process::CommandExt as _;
use std::path::PathBuf;
use std::sync::mpsc;

use tray_icon::menu::{CheckMenuItem, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, SetLastError, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW,
    GetWindowLongPtrW, HWND_MESSAGE, KillTimer, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SetTimer, SetWindowLongPtrW, TranslateMessage, WM_APP, WM_TIMER, WNDCLASSW,
};

use crate::api_error::StartupError;
use crate::runner::{GatewayHandle, ServeOptions, run_headless, spawn};
use crate::tray::logic::{self, TrayPhase};
use crate::tray::menu::{BuiltMenu, MenuBuildError};

/// The window message that asks the loop to drain the forwarded events.
const WM_TRAY_EVENT: u32 = WM_APP + 1;

/// The status-refresh timer.
const STATUS_TIMER: usize = 1;

/// The status-refresh cadence: the label, tooltip, icon phase, Workshop
/// enabled bit, and login check all re-read their sources on this timer,
/// never from one-shot events.
const STATUS_INTERVAL_MS: u32 = 5_000;

/// The tray icon's edge length in pixels; the embedded asset is 32x32
/// RGBA.
const ICON_SIZE: u32 = 32;

/// The brand icon as raw RGBA, derived from the workshop's `32x32.png`
/// brand asset (PIL: `Image.open(...).convert("RGBA").tobytes()`;
/// regenerate from `crates/workshop/icons/32x32.png` when the brand
/// changes).
const BRAND_RGBA: &[u8] = include_bytes!("../../assets/tray-icon.rgba");

// The asset is exactly one 32x32 RGBA image.
const _: () = assert!(BRAND_RGBA.len() == (ICON_SIZE * ICON_SIZE * 4) as usize);

thread_local! {
    /// Set while the menu's modal `TrackPopupMenu` loop runs. The modal
    /// loop dispatches this thread's messages reentrantly - including this
    /// window's timer and wake messages - so the window procedure skips
    /// event and timer handling while it is up: a nested dispatch must
    /// never create a second `&mut Tray` aliasing the one the menu call
    /// holds. Queued events drain when the outer dispatch resumes, and the
    /// timer is memoryless, so skipping loses nothing.
    static MENU_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// Spawns the gateway, then runs the tray on the main thread until Quit.
/// A tray that cannot start degrades to the headless Ctrl-C loop: the
/// gateway is already serving and the tray is its face, not its life
/// support.
pub(super) fn run(options: &ServeOptions) -> Result<(), StartupError> {
    let handle = spawn(options)?;
    let tray = match Tray::build(&handle) {
        Ok(tray) => tray,
        Err(error) => {
            tracing::error!("the system tray is unavailable: {error}; running headless");
            return run_headless(handle);
        }
    };
    tray.arm(handle).run()
}

/// Why the tray could not start; [`run`] falls back to headless serving.
#[derive(Debug, thiserror::Error)]
enum TrayError {
    /// The hidden message window could not be created.
    #[error("create the tray message window")]
    Window(#[source] std::io::Error),
    /// The embedded icon asset did not decode.
    #[error("decode the tray icon")]
    Icon(#[source] tray_icon::BadIcon),
    /// The menu could not be assembled.
    #[error(transparent)]
    Menu(#[from] MenuBuildError),
    /// The tray icon could not be registered with the shell.
    #[error("register the tray icon")]
    Register(#[source] tray_icon::Error),
}

/// An event forwarded from a tray-icon or muda callback into the message
/// loop. The callbacks run inside the loop's dispatch; forwarding keeps
/// them out of every action that does real work.
enum TrayEvent {
    /// A menu item was activated.
    Menu(MenuId),
    /// The right button was released on the icon: re-probe, then show the
    /// menu.
    RightClick,
    /// The left button was double-clicked: open Settings.
    LeftDoubleClick,
}

/// The three phase icons, precomputed once from the embedded asset.
struct PhaseIcons {
    starting: Icon,
    running: Icon,
    error: Icon,
}

impl PhaseIcons {
    /// Decodes the embedded asset and its grayed and error variants.
    fn load() -> Result<PhaseIcons, TrayError> {
        let decode =
            |rgba: Vec<u8>| Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).map_err(TrayError::Icon);
        Ok(PhaseIcons {
            starting: decode(logic::grayed(BRAND_RGBA))?,
            running: decode(BRAND_RGBA.to_vec())?,
            error: decode(logic::error_tint(BRAND_RGBA))?,
        })
    }

    /// The icon for one phase.
    fn for_phase(&self, phase: TrayPhase) -> &Icon {
        match phase {
            TrayPhase::Starting => &self.starting,
            TrayPhase::Running => &self.running,
            TrayPhase::Error => &self.error,
        }
    }
}

/// The launch-at-login store backed by the HKCU Run key, through the
/// registry shims in `crate::boot`.
struct WindowsRunKey;

impl logic::RunKeyStore for WindowsRunKey {
    fn read(&self) -> Option<String> {
        crate::boot::registry::read_run_value()
    }

    fn write(&mut self, command: &str) -> std::io::Result<()> {
        crate::boot::registry::write_run_value(command)
    }

    fn delete(&mut self) -> std::io::Result<()> {
        crate::boot::registry::delete_run_value()
    }
}

/// The tray's whole state. Once armed, it lives behind the window's
/// `GWLP_USERDATA` and is touched only on the main thread.
struct Tray {
    /// The tray's own message window.
    hwnd: HWND,
    /// Forwarded events, drained on every `WM_TRAY_EVENT`.
    events: mpsc::Receiver<TrayEvent>,
    /// Set by the Quit action; the loop checks it after every dispatch
    /// because a modal menu loop can consume `WM_QUIT`.
    quit: bool,
    /// The tray icon; `None` after teardown takes it. Never cloned: the
    /// icon is reference-counted and a surviving clone leaks a ghost icon
    /// in the notification area.
    icon: Option<TrayIcon>,
    /// The per-phase icons, precomputed from the embedded asset.
    icons: PhaseIcons,
    /// The current icon phase.
    phase: TrayPhase,
    /// The last rendered status text, so an unchanged tick skips the shell
    /// updates.
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
    /// The running gateway: the status source and the shutdown switch.
    /// `None` only while teardown takes it.
    handle: Option<GatewayHandle>,
    /// The one-time browser handoff URL for Settings and double-click.
    auth_url: String,
    /// The workshop exe beside ours, when present.
    workshop_exe: Option<PathBuf>,
}

impl Tray {
    /// Arms the tray: stores the gateway handle, moves the tray behind the
    /// window's user data for the loop's whole lifetime, runs the first
    /// status refresh, and sets the refresh timer. Infallible: every
    /// fallible step happened in [`Tray::build`], so a failure there can
    /// hand the handle back to the headless fallback.
    fn arm(mut self, handle: GatewayHandle) -> Armed {
        self.handle = Some(handle);
        let hwnd = self.hwnd;
        // The tray moves behind the window's user data for the loop's
        // whole lifetime; teardown reclaims it exactly once.
        let ptr = Box::into_raw(Box::new(self));
        // SAFETY: `hwnd` is the live window built above and `ptr` is a
        // live Tray that outlives every dispatch; both stay on this
        // thread.
        unsafe {
            SetLastError(0);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
            if GetLastError() != 0 {
                // A live window's user-data slot cannot fail to store; if
                // it somehow does, the loop runs deaf, so say so.
                tracing::error!("could not install the tray state on the message window");
            }
        }
        // SAFETY: `ptr` is the live Tray installed above, on this thread.
        unsafe {
            tick(&mut *ptr);
        }
        // SAFETY: `hwnd` is live; arms the refresh timer on this thread's
        // queue.
        let timer = unsafe { SetTimer(hwnd, STATUS_TIMER, STATUS_INTERVAL_MS, None) };
        if timer == 0 {
            // The tray still works; only the refresh stops. Say so rather
            // than silently freezing the status line.
            tracing::warn!("could not arm the tray status timer; the status line will not refresh");
        }
        Armed { hwnd }
    }

    /// Everything fallible, before the tray is armed: the icons, the menu,
    /// the window, the event forwarding, and the shell registration. The
    /// gateway handle is only read, so a failure can hand it back.
    fn build(handle: &GatewayHandle) -> Result<Tray, TrayError> {
        let icons = PhaseIcons::load()?;
        let auth_url = logic::auth_url(handle.url(), handle.tray_key());
        let workshop_exe = probe_workshop();
        let login_checked = logic::launch_at_login(&WindowsRunKey);
        let label = logic::status_label(TrayPhase::Starting, 0, 0.0);
        // The HKCU Run key is always available, so the item is always enabled.
        let spec = logic::menu_spec(&label, workshop_exe.is_some(), true, login_checked);
        let menu = BuiltMenu::from_spec(&spec)?;
        let (events, rx) = mpsc::channel();
        let hwnd = create_window()?;
        forward_events(events, hwnd);
        let icon = match TrayIconBuilder::new()
            .with_menu(Box::new(menu.menu))
            .with_tooltip(&label)
            .with_icon(icons.starting.clone())
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(false)
            .build()
        {
            Ok(icon) => icon,
            Err(error) => {
                // SAFETY: `hwnd` was created above on this thread and is
                // destroyed on it.
                unsafe {
                    DestroyWindow(hwnd);
                }
                return Err(TrayError::Register(error));
            }
        };
        Ok(Tray {
            hwnd,
            events: rx,
            quit: false,
            icon: Some(icon),
            icons,
            phase: TrayPhase::Starting,
            label,
            status_item: menu.status,
            workshop_item: menu.workshop,
            settings_item: menu.settings,
            login_item: menu.login,
            quit_item: menu.quit,
            handle: None,
            auth_url,
            workshop_exe,
        })
    }
}

/// The armed tray: window created, icon registered, timer set.
struct Armed {
    hwnd: HWND,
}

impl Armed {
    /// Pumps messages until Quit, then tears down: the tray icon first (a
    /// surviving reference leaks a ghost icon in the notification area),
    /// then the window, then the gateway's graceful shutdown - never
    /// `process::exit` ahead of destructors, so the connection-file guard
    /// still runs.
    fn run(self) -> Result<(), StartupError> {
        message_loop(self.hwnd);
        // SAFETY: `hwnd` is our window; its timer is ours.
        unsafe {
            KillTimer(self.hwnd, STATUS_TIMER);
        }
        let mut tray = reclaim(self.hwnd);
        drop(tray.icon.take());
        // SAFETY: `hwnd` was created and is destroyed on this thread.
        unsafe {
            DestroyWindow(self.hwnd);
        }
        let handle = tray.handle.take();
        drop(tray);
        match handle {
            Some(handle) => handle.shutdown(),
            None => Ok(()),
        }
    }
}

/// The Tray behind a window's user data, or null before installation.
fn tray_ptr(hwnd: HWND) -> *mut Tray {
    // SAFETY: reads this window's user data; no aliasing.
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Tray }
}

/// Reclaims the Tray installed by `Tray::arm`.
fn reclaim(hwnd: HWND) -> Box<Tray> {
    // SAFETY: the Tray was installed once by `Tray::arm`, the loop has
    // exited, and this is the only reclamation.
    unsafe { Box::from_raw(tray_ptr(hwnd)) }
}

/// Creates the hidden message-only window whose loop the main thread
/// runs.
fn create_window() -> Result<HWND, TrayError> {
    let class_name: Vec<u16> = "promptforge-gateway-tray"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: a null module handle names this process's module.
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let wnd_class = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        lpszClassName: class_name.as_ptr(),
        hInstance: hinstance,
        // SAFETY: WNDCLASSW is plain old data; zeroed is a valid default.
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: `wnd_class` is valid and `class_name` outlives the call. One
    // process runs one tray, so the class is registered once.
    let atom = unsafe { RegisterClassW(&raw const wnd_class) };
    if atom == 0 {
        return Err(TrayError::Window(std::io::Error::last_os_error()));
    }
    // SAFETY: the class was just registered; a message-only window is
    // never visible and never in the taskbar.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null_mut(),
        )
    };
    if hwnd.is_null() {
        return Err(TrayError::Window(std::io::Error::last_os_error()));
    }
    Ok(hwnd)
}

/// Installs the tray-icon and menu callbacks. Each callback only forwards
/// the event into the loop's queue and posts the wake message; the loop
/// does the work, so the callback path never blocks on process launches,
/// browser opens, or registry writes.
fn forward_events(events: mpsc::Sender<TrayEvent>, hwnd: HWND) {
    // A raw window handle is not Send; its address is. The address is
    // valid for the window's lifetime, and a post after teardown fails
    // harmlessly because the loop is already gone.
    let hwnd_addr = hwnd as usize;
    let tray_events = events.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        let forwarded = match event {
            TrayIconEvent::Click {
                button: MouseButton::Right,
                button_state: MouseButtonState::Up,
                ..
            } => Some(TrayEvent::RightClick),
            TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => Some(TrayEvent::LeftDoubleClick),
            _ => None,
        };
        if let Some(event) = forwarded {
            forward(&tray_events, hwnd_addr, event);
        }
    }));
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        forward(&events, hwnd_addr, TrayEvent::Menu(event.id));
    }));
}

/// Sends one event into the loop's queue and posts the wake message.
fn forward(events: &mpsc::Sender<TrayEvent>, hwnd_addr: usize, event: TrayEvent) {
    if events.send(event).is_ok() {
        // SAFETY: the address is the tray's message window; a post after
        // the window is destroyed fails without effect.
        unsafe {
            PostMessageW(hwnd_addr as HWND, WM_TRAY_EVENT, 0, 0);
        }
    }
}

/// The message window's procedure: drains forwarded events on
/// `WM_TRAY_EVENT` and refreshes the status on the timer.
///
/// # Safety
/// Windows calls this for the window's messages, always on the window's
/// thread.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY_EVENT => {
            let ptr = tray_ptr(hwnd);
            // The MENU_OPEN guard keeps a reentrant dispatch from the
            // menu's modal loop out of the Tray the outer dispatch holds.
            if !ptr.is_null() && !MENU_OPEN.with(Cell::get) {
                // SAFETY: the installed Tray is live until teardown, the
                // window is only ever dispatched on this thread, and no
                // other `&mut Tray` is live while the menu is closed.
                let tray = unsafe { &mut *ptr };
                while let Ok(event) = tray.events.try_recv() {
                    act(tray, event);
                }
            }
            0
        }
        WM_TIMER if wparam == STATUS_TIMER => {
            let ptr = tray_ptr(hwnd);
            if !ptr.is_null() && !MENU_OPEN.with(Cell::get) {
                // SAFETY: as above.
                tick(unsafe { &mut *ptr });
            }
            0
        }
        // SAFETY: unhandled messages take the default behavior.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The main thread's message loop. `GetMessageW` returns 0 on `WM_QUIT`
/// and -1 on error; the Quit action's flag is also checked after every
/// dispatch because a modal menu loop can consume the `WM_QUIT`.
fn message_loop(hwnd: HWND) {
    loop {
        let mut msg = MSG::default();
        // SAFETY: `msg` is valid for one MSG write; a null window handle
        // retrieves every message on this thread's queue.
        let result = unsafe { GetMessageW(&raw mut msg, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        // SAFETY: `msg` was filled by the `GetMessageW` above.
        unsafe {
            TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
        let ptr = tray_ptr(hwnd);
        if !ptr.is_null() {
            // SAFETY: the installed Tray is live until teardown.
            if unsafe { (*ptr).quit } {
                break;
            }
        }
    }
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
        // follows it through the normal teardown. Without this the
        // message loop would pump a dead gateway forever, showing a false
        // error. A failure's StartupError still propagates through the
        // join in teardown.
        request_quit(tray);
        return;
    }
    let phase = logic::next_phase(poll);
    if phase != tray.phase {
        tray.phase = phase;
        if let Some(icon) = tray.icon.as_ref()
            && let Err(error) = icon.set_icon(Some(tray.icons.for_phase(phase).clone()))
        {
            // A failed NIM_MODIFY is retried on the next transition.
            tracing::debug!("could not update the tray icon: {error}");
        }
    }
    if let Some((models, vram_gb)) = handle.tray_state().tray_model_status() {
        let label = logic::status_label(phase, models, vram_gb);
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
    tray.workshop_exe = probe_workshop();
    tray.workshop_item.set_enabled(tray.workshop_exe.is_some());
    tray.login_item
        .set_checked(logic::launch_at_login(&WindowsRunKey));
}

/// One forwarded event, handled on the loop thread.
fn act(tray: &mut Tray, event: TrayEvent) {
    match event {
        TrayEvent::RightClick => show_menu(tray),
        TrayEvent::LeftDoubleClick => open_settings(tray),
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

/// Opens the menu after re-probing the Workshop sibling and re-reading
/// the login state, so a displayed menu never shows a stale enabled bit
/// or check mark. The crate's automatic shows are disabled because muda's
/// items are `!Send` and the re-probe cannot run inside the event
/// callback ahead of them.
fn show_menu(tray: &mut Tray) {
    tray.workshop_exe = probe_workshop();
    tray.workshop_item.set_enabled(tray.workshop_exe.is_some());
    tray.login_item
        .set_checked(logic::launch_at_login(&WindowsRunKey));
    if let Some(icon) = tray.icon.as_ref() {
        // TrackPopupMenu runs a modal loop that dispatches this thread's
        // messages reentrantly; the guard keeps the nested dispatch out of
        // the Tray while this call holds it.
        MENU_OPEN.with(|open| open.set(true));
        icon.show_menu();
        MENU_OPEN.with(|open| open.set(false));
    }
}

/// Opens the config SPA in the default browser through the one-time
/// handoff URL, so the bearer key never sits in browser history.
fn open_settings(tray: &Tray) {
    if let Err(error) = open::that(&tray.auth_url) {
        tracing::warn!("could not open the settings page: {error}");
    }
}

/// Launches the workshop shell, detached: it attaches to this gateway
/// through the connection file and outlives it.
fn launch_workshop(tray: &Tray) {
    // The same detach the shell uses for its own gateway spawn
    // (crates/workshop/src/gateway.rs): broken out of any job object whose
    // kill-on-close would reap the workshop with the gateway, no inherited
    // stdio, and a new process group.
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let Some(exe) = tray.workshop_exe.as_ref() else {
        return;
    };
    let mut command = std::process::Command::new(exe);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    if let Err(error) = command.spawn() {
        tracing::warn!("could not launch {}: {error}", exe.display());
    }
}

/// Toggles the launch-at-login entry in the OS and reflects the result in
/// the check item. A failed write leaves the check showing the OS state.
fn toggle_login(tray: &mut Tray) {
    let mut store = WindowsRunKey;
    let enable = !logic::launch_at_login(&store);
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            tracing::warn!("could not locate the gateway executable: {error}");
            return;
        }
    };
    match logic::set_launch_at_login(&mut store, &exe, enable) {
        Ok(enabled) => tray.login_item.set_checked(enabled),
        Err(error) => {
            tracing::warn!("could not update the login entry: {error}");
            // muda toggles the check on click, before the event arrives;
            // a failed write restores the OS state immediately rather
            // than waiting for the next tick.
            tray.login_item.set_checked(logic::launch_at_login(&store));
        }
    }
}

/// Ends the message loop. The flag is the authoritative signal; the
/// posted `WM_QUIT` only wakes `GetMessageW`, and a modal menu loop may
/// consume it.
fn request_quit(tray: &mut Tray) {
    tray.quit = true;
    // SAFETY: posts `WM_QUIT` to this thread's queue.
    unsafe {
        PostQuitMessage(0);
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
