//! What the run loop does on the main thread: the status refresh, the
//! menu's pre-display re-probe, and the menu items' actions.

use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;

use super::{Tray, TrayEvent};
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
        // follows it through the normal teardown. Without this the run
        // loop would pump a dead gateway forever, showing a false error.
        request_quit(tray);
        return;
    }
    tray.phase = logic::next_phase(poll);
    let busy = handle.tray_state().tray_busy_text();
    if let Some((models, vram_gb)) = handle.tray_state().tray_model_status() {
        let label = logic::status_label(tray.phase, busy.as_deref(), models, vram_gb);
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
pub(super) fn act(tray: &mut Tray, event: TrayEvent) {
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
/// gateway discovery file and outlives it. An unbundled dev run spawns the
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
        // gateway spawn does (crates/workshop/desktop/src/gateway.rs): its own
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
pub(super) fn probe_workshop() -> Option<PathBuf> {
    match std::env::current_exe() {
        Ok(exe) => logic::workshop_sibling(&exe),
        Err(error) => {
            tracing::warn!("could not locate the gateway executable: {error}");
            None
        }
    }
}
