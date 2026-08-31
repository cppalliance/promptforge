//! The PromptForge Workshop desktop shell: the window, the webview, and
//! the platform bridges behind one narrow entry point.
//!
//! This crate owns the tao event loop, the wry webview pointed at the
//! hosted workshop UI, the custom-title-bar IPC commands, the navigation
//! policy (same-origin loads in place, everything else opens in the
//! system browser), the microphone permission grant, Explorer file drops,
//! and the program icon. On Windows it also owns the WebView2 web-message
//! bridge that recovers real OS paths from dropped files - the
//! workspace's only unsafe code.
//!
//! The desktop binary (`promptforge-workshop`) keeps lifecycle orchestration -
//! configuration discovery, gateway start, the health wait, and
//! shutdown - and drives this crate through [`run`], the entire public
//! surface.

// The only unsafe module in the workspace: the WebView2 COM surface that
// reads real OS paths out of dropped File objects has no safe wrapper.
// The clippy allows cover code the #[implement] macro expands in tests.
#[cfg(target_os = "windows")]
#[allow(unsafe_code, clippy::inline_always, clippy::ref_as_ptr)]
mod file_drop;
mod window;

pub use window::run;
