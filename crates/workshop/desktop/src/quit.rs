//! The quit-everything gesture, shared by the native menu's quit item and
//! the SPA's File > Exit row (the `quit` command): one
//! stop-then-shutdown-then-exit path so the two cannot diverge. The path
//! first stops the local-sidecar supervisor, so nothing relaunches the
//! gateway this gesture is about to stop. When boot attached to or
//! launched a local sidecar gateway, the path then posts the gateway's
//! `/shutdown` through the server's current validated Gateway snapshot, so
//! one gesture stops the window, the in-process server, and the Gateway.
//! Attached to a LAN Gateway through explicit config, there is no
//! supervisor and the snapshot grants no shutdown authority, so the gesture
//! stops the desktop app only.

use std::sync::PoisonError;

use tauri::{AppHandle, Manager as _, Wry};
use workshop_server_api::GatewayUpdater;

use crate::gateway::GatewaySupervisor;
use crate::{GatewaySupervisorSlot, ServerSlot};

/// The quit ordering: stop the supervisor, then request the gateway's
/// shutdown. The reverse order races: a supervisor still running when the
/// gateway stops sees it missing and launches a replacement.
pub(crate) fn stop_supervisor_then_request_shutdown(
    stop_supervisor: impl FnOnce(),
    request_shutdown: impl FnOnce(),
) {
    stop_supervisor();
    request_shutdown();
}

/// The supervisor-stop half: takes the local-sidecar supervisor out of its
/// slot and shuts it down, reporting a worker that panicked or outlived its
/// budget. The `RunEvent::Exit` handler then finds the slot empty.
fn stop_supervisor(app: &AppHandle<Wry>) {
    let supervisor = app
        .try_state::<GatewaySupervisorSlot>()
        .and_then(|slot| slot.lock().unwrap_or_else(PoisonError::into_inner).take());
    crate::report_supervisor_shutdown(supervisor.map(GatewaySupervisor::shutdown));
}

/// The shutdown-request half: asks the server's current validated local
/// Gateway snapshot to post `/shutdown`. A configured LAN Gateway grants
/// no shutdown authority, so its snapshot sends nothing. A refused or
/// undeliverable request is reported and quit proceeds anyway - quit
/// always works, even when the Gateway is wedged.
pub(crate) fn request_gateway_shutdown(gateway: Option<GatewayUpdater>) {
    if let Some(gateway) = gateway
        && let Err(error) = gateway.request_shutdown()
    {
        eprintln!(
            "the gateway did not accept the shutdown request; quitting the desktop app anyway: {error}"
        );
    }
}

/// The shared stop-then-shutdown-then-exit path: stop the supervisor,
/// request the local Gateway's shutdown, then exit the desktop app (the
/// `RunEvent::Exit` handler stops the in-process server).
pub(crate) fn quit_everything(app: &AppHandle<Wry>) {
    stop_supervisor_then_request_shutdown(
        || stop_supervisor(app),
        || {
            let gateway = app.try_state::<ServerSlot>().and_then(|slot| {
                slot.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .as_ref()
                    .map(workshop_server_api::ServerHandle::gateway_updater)
            });
            request_gateway_shutdown(gateway);
        },
    );
    app.exit(0);
}

/// The `quit` command behind the SPA's File > Exit row. It runs the same
/// stop-then-shutdown-then-exit path as the native menu's quit item so the
/// two gestures cannot diverge; a plain `process.exit` from the webview
/// would strand the sidecar gateway.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "a Tauri command handler receives the app handle by value"
)]
pub(crate) fn quit(app: AppHandle<Wry>) {
    quit_everything(&app);
}

#[cfg(test)]
#[path = "quit-tests.rs"]
mod tests;
