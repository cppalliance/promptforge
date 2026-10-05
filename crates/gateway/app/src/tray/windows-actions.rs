//! What the loop does on the main thread: the status refresh, the
//! forwarded events, and the menu items' actions.

use std::os::windows::process::CommandExt as _;
use std::path::PathBuf;

use windows_sys::Win32::UI::WindowsAndMessaging::PostQuitMessage;

use super::{MENU_OPEN, Tray, TrayEvent, WindowsRunKey};
use crate::tray::logic;

/// The status refresh: recomputes the phase from the gateway thread's
/// liveness and the label from the gateway's in-process state, and keeps
/// the Workshop enabled bit and the login check honest. Nothing here
/// blocks: a profile switch holding the live-state lock simply skips the
/// label update for one tick.
pub(super) fn tick(tray: &mut Tray) {
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
    let busy = handle.tray_state().tray_busy_text();
    if let Some((models, vram_gb)) = handle.tray_state().tray_model_status() {
        let label = logic::status_label(phase, busy.as_deref(), models, vram_gb);
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
pub(super) fn act(tray: &mut Tray, event: TrayEvent) {
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
        // the Tray while this call holds it. The crate's own
        // SetForegroundWindow + WM_NULL pairing (KB Q135788) handles the
        // foreground and dismissal.
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
/// through the gateway discovery file and outlives it.
fn launch_workshop(tray: &Tray) {
    // The same detach the shell uses for its own gateway spawn
    // (crates/workshop/desktop/src/gateway.rs): broken out of any job object whose
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
    match logic::set_launch_at_login(&mut store, &logic::run_key_command(&exe), enable) {
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
pub(super) fn probe_workshop() -> Option<PathBuf> {
    match std::env::current_exe() {
        Ok(exe) => logic::workshop_sibling(&exe),
        Err(error) => {
            tracing::warn!("could not locate the gateway executable: {error}");
            None
        }
    }
}
