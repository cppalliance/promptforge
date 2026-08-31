//! Linux microphone permission for the workshop window.
//!
//! WebKitGTK ships two defaults that together break voice dictation:
//! `enable-media-stream` is off, and the stock `permission-request` handler
//! denies every request it never saw a listener override, including
//! microphone/camera grabs. Neither is fixable from outside the embedding
//! application, so the session opts in here and auto-allows only user-media
//! requests - every other permission kind (notifications, geolocation, ...)
//! keeps the default deny.

use anyhow::Context as _;
use webkit2gtk::{
    PermissionRequestExt as _, SettingsExt as _, UserMediaPermissionRequest, WebViewExt as _,
    glib::Cast as _,
};

/// Enables media capture on the window's webview and connects the
/// permission handler for the webview's lifetime.
///
/// # Errors
/// Returns an error when the closure cannot be dispatched to the webview's
/// thread; the caller logs it and the mic stays unavailable, nothing else
/// breaks.
pub(crate) fn grant_media_permissions(window: &tauri::WebviewWindow) -> anyhow::Result<()> {
    window
        .with_webview(|webview| {
            let webview = webview.inner();
            if let Some(settings) = webview.settings() {
                settings.set_enable_media_stream(true);
            }
            webview.connect_permission_request(|_webview, request| match request
                .downcast_ref::<UserMediaPermissionRequest>(
            ) {
                Some(request) => {
                    request.allow();
                    true
                }
                None => false,
            });
        })
        .context("dispatch the media permission setup to the webview thread")
}
