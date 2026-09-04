//! The system tray: the gateway's only on-screen presence.
//!
//! The gateway has no window - its config SPA is its face - so on an
//! installed system the tray icon is the daemon's UI. The tray owns the
//! main thread through a per-OS backend ([`windows`], [`macos`],
//! [`linux`]), while the tokio runtime and serving stay on the gateway
//! thread spawned by [`crate::spawn`]. The platform-independent rules -
//! the menu layout, the status label, the icon phase machine, the
//! launch-at-login entry - live in [`logic`] so the idiom cannot drift
//! between platforms, and the muda menu materialization lives in [`menu`]
//! for the same reason (Linux materializes through ksni's own menu API).
//!
//! [`run_with_tray`] is the binary's default main loop; `--no-tray` keeps
//! the headless Ctrl-C loop ([`crate::run`]) for servers and CI.

use crate::api_error::StartupError;
use crate::runner::ServeOptions;

// Compiled for every backend platform and for tests everywhere: the rules
// are pure logic, and the test suite exercises them on headless CI.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", test))]
pub(crate) mod logic;
// The Linux backend: a pure StatusNotifierItem over the session D-Bus via
// ksni, no GTK. Safe throughout: the D-Bus boundary is ksni's, not ours.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "SMAppService registration and NSTimer block scheduling are raw Objective-C messaging with no safe wrapper"
)]
mod macos;
// The muda menu materialization shared by the tray-icon backends; not
// compiled under cfg(test) off-platform because tray-icon only exists in
// Windows and macOS builds.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) mod menu;
#[cfg(target_os = "windows")]
#[expect(
    unsafe_code,
    reason = "the hidden-window message loop is raw Win32 with no safe wrapper"
)]
mod windows;

/// Runs the gateway with the system tray owning the main thread.
///
/// On Windows the tray's hidden-window message loop is the main loop and
/// serving stays on the gateway thread; on macOS the NSApplication run
/// loop owns the main thread; on Linux the tray drives the D-Bus service
/// from its own current-thread runtime on the main thread. On platforms
/// without a backend, this falls back to the headless Ctrl-C loop with a
/// warning.
///
/// # Errors
/// Returns [`StartupError`] when config loading, provisioning, binding, or
/// serving fails; classify with [`StartupError::kind`].
pub fn run_with_tray(options: &ServeOptions) -> Result<(), StartupError> {
    run_inner(options)
}

#[cfg(target_os = "windows")]
fn run_inner(options: &ServeOptions) -> Result<(), StartupError> {
    windows::run(options)
}

#[cfg(target_os = "macos")]
fn run_inner(options: &ServeOptions) -> Result<(), StartupError> {
    macos::run(options)
}

#[cfg(target_os = "linux")]
fn run_inner(options: &ServeOptions) -> Result<(), StartupError> {
    linux::run(options)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn run_inner(options: &ServeOptions) -> Result<(), StartupError> {
    tracing::warn!("the system tray has no backend on this platform; running headless");
    crate::run(options)
}
