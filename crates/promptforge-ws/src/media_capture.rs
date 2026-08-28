//! Loopback media capture enablement for the macOS webview.
//!
//! WebKit gates `navigator.mediaDevices` behind its
//! `mediaCaptureRequiresSecureConnection` preference, which admits only
//! https origins. Unlike WebView2 it has no loopback exemption, so the
//! workshop's `http://127.0.0.1` origin ships with voice capture hidden
//! and the mic's feature check fails. The preference is only reachable
//! through WebKit SPI (`_setMediaCaptureRequiresSecureConnection:`), the
//! same switch behind Safari's "allow media capture on insecure sites"
//! develop toggle.
//!
//! The preference must be in place before the webview exists:
//! `WKWebView` copies its configuration at init, so a flip on the
//! already-built webview's `configuration` mutates the copy and never
//! reaches the page. The shell therefore builds the
//! `WKWebViewConfiguration` itself, clears the gate, and hands the
//! configuration to wry. The SPI call is guarded by
//! `respondsToSelector:`, so a WebKit that drops the SPI leaves voice in
//! the UI's existing "not available" degraded state rather than
//! crashing.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_web_kit::WKWebViewConfiguration;

/// Byte length of the embedded Info.plist, named so the section static
/// below can spell its array type.
const INFO_PLIST_LEN: usize = include_bytes!("Info.plist").len();

/// The shell's Info.plist, embedded in the executable's
/// `__TEXT,__info_plist` section: the linker convention macOS reads for
/// non-bundled binaries (`NSBundle.mainBundle` and TCC both consult it).
/// WKWebView exposes `navigator.mediaDevices` only when the host app
/// declares `NSMicrophoneUsageDescription`, so without this section the
/// mic's feature check fails in the page no matter what the WebKit
/// preferences say.
#[used]
#[unsafe(link_section = "__TEXT,__info_plist")]
static INFO_PLIST: [u8; INFO_PLIST_LEN] = *include_bytes!("Info.plist");

/// Builds a webview configuration with WebKit's secure-connection media
/// capture requirement cleared, so the plain-http loopback origin gets
/// `navigator.mediaDevices`.
///
/// Returns `None` off the main thread (`WKWebViewConfiguration` is
/// main-thread-only) or when the SPI is gone from WebKit; the caller
/// then builds the webview with wry's own configuration and voice
/// capture stays unavailable in the page.
pub(crate) fn insecure_capture_configuration() -> Option<Retained<WKWebViewConfiguration>> {
    let mtm = MainThreadMarker::new()?;
    // SAFETY: a fresh WKWebViewConfiguration on the main thread is the
    // documented construction; `preferences` is a documented accessor
    // returning a retained object; the SPI setter takes one BOOL and is
    // probed with respondsToSelector before it is sent.
    unsafe {
        let configuration = WKWebViewConfiguration::new(mtm);
        let preferences: Retained<AnyObject> = msg_send![&*configuration, preferences];
        let setter = sel!(_setMediaCaptureRequiresSecureConnection:);
        let responds: bool = msg_send![&*preferences, respondsToSelector: setter];
        if !responds {
            return None;
        }
        let () = msg_send![&*preferences, _setMediaCaptureRequiresSecureConnection: false];
        Some(configuration)
    }
}
