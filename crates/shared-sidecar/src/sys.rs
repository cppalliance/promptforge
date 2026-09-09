//! Process identity lookup for stale detection: one shim per platform,
//! each answering "what binary does this pid run, and which process boot
//! owns the pid", so pid reuse cannot join separate validation observations.

use std::path::PathBuf;

/// An OS-observed process boot, stable for one lifetime and different
/// when the pid is reused.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProcessIdentity {
    /// The process executable observed for this boot.
    pub(crate) image: PathBuf,
    /// Platform start marker: FILETIME, proc start ticks, or timeval.
    started: u128,
}

impl ProcessIdentity {
    /// Assembles one platform observation.
    pub(crate) const fn new(image: PathBuf, started: u128) -> Self {
        Self { image, started }
    }

    /// Assembles a deterministic identity for validation regressions.
    #[cfg(test)]
    pub(crate) const fn for_test(image: PathBuf, started: u128) -> Self {
        Self::new(image, started)
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
#[expect(
    unsafe_code,
    reason = "proc_pidpath and proc_pidinfo are raw C APIs with no safe wrappers"
)]
mod macos;
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "process identity uses raw Win32 handle and query APIs"
)]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::process_identity;
#[cfg(target_os = "macos")]
pub(crate) use macos::process_identity;
#[cfg(windows)]
pub(crate) use windows::process_identity;

/// Every other platform fails closed: no process identity means the
/// gateway discovery file is always stale, so a reader relaunches rather than
/// attaching to an unverified process.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) fn process_identity(_pid: u32) -> Option<ProcessIdentity> {
    None
}
