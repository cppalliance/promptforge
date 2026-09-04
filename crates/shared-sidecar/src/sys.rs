//! Process image lookup for stale detection: one shim per platform, each
//! answering "what binary does this pid run", so a reused pid cannot
//! impersonate the gateway that wrote a connection file. A live answer
//! doubles as the liveness check: a dead pid has no image to query.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "proc_pidpath is a raw C API with no safe wrapper"
)]
mod macos;
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "OpenProcess and QueryFullProcessImageNameW are raw Win32 with no safe wrapper"
)]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::process_image_path;
#[cfg(target_os = "macos")]
pub(crate) use macos::process_image_path;
#[cfg(windows)]
pub(crate) use windows::process_image_path;

/// Every other platform fails closed: no image answer means the connection
/// file is always treated as stale, so a reader relaunches rather than
/// attaching to an unverified process.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) fn process_image_path(_pid: u32) -> Option<std::path::PathBuf> {
    None
}
