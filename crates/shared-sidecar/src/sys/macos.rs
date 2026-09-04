//! macOS process image lookup via `proc_pidpath` (libproc, part of
//! libSystem): one call answers both liveness and identity - a dead pid
//! has no image to report.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;

/// Buffer size for `proc_pidpath`: `PROC_PIDPATHINFO_MAXSIZE` from
/// `libproc.h` (4 * MAXPATHLEN).
const PROC_PIDPATHINFO_MAXSIZE: u32 = 4096;

/// The kernel's path for the process's executable, or `None` when the
/// process is gone or refuses the query.
pub(crate) fn process_image_path(pid: u32) -> Option<PathBuf> {
    let pid = i32::try_from(pid).ok()?;
    let mut buffer = vec![0u8; PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buffer` is a live allocation of exactly
    // PROC_PIDPATHINFO_MAXSIZE bytes and the size handed over matches it;
    // proc_pidpath writes at most that many bytes and returns the count
    // written, or a value <= 0 on error.
    let written =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), PROC_PIDPATHINFO_MAXSIZE) };
    if written <= 0 {
        return None;
    }
    let written = usize::try_from(written).ok()?;
    buffer.truncate(written);
    Some(PathBuf::from(OsString::from_vec(buffer)))
}
