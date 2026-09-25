//! The quit-everything gesture, shared by the native menu's quit item and
//! the SPA's File > Exit row (the `quit` command): one
//! shutdown-then-exit path so the two cannot diverge. When boot attached
//! to or launched a local sidecar gateway, the path first posts the
//! gateway's `/shutdown` through the server's current validated Gateway
//! snapshot, so one gesture stops the window, the in-process server, and
//! the Gateway. Attached to a LAN Gateway through explicit config, the
//! snapshot grants no shutdown authority, so the gesture stops the desktop app
//! only.

use std::sync::PoisonError;

use tauri::{AppHandle, Manager as _, Wry};
use workshop_server_api::GatewayUpdater;

use crate::ServerSlot;

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

/// The shared shutdown-then-exit path: request the local Gateway's
/// shutdown, then exit the desktop app (the `RunEvent::Exit` handler stops the
/// in-process server).
pub(crate) fn quit_everything(app: &AppHandle<Wry>) {
    let gateway = app.try_state::<ServerSlot>().and_then(|slot| {
        slot.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(workshop_server_api::ServerHandle::gateway_updater)
    });
    request_gateway_shutdown(gateway);
    app.exit(0);
}

/// The `quit` command behind the SPA's File > Exit row. It runs the same
/// shutdown-then-exit path as the native menu's quit item so the two
/// gestures cannot diverge; a plain `process.exit` from the webview would
/// strand the sidecar gateway.
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
