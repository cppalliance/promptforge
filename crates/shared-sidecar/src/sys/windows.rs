//! Windows process identity lookup: one process handle supplies the image
//! and creation FILETIME, so pid reuse cannot join two observations.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt as _;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

use super::ProcessIdentity;

/// The image and creation time of process `pid`, or `None` when the
/// process is dead or refuses a limited query.
pub(crate) fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    // SAFETY: `OpenProcess` takes a valid access mask and pid; the
    // returned handle is either null (checked) or a live process handle
    // that `CloseHandle` below releases exactly once.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let identity = query_identity(handle);
    // SAFETY: `handle` is the live process handle returned by the
    // `OpenProcess` above, closed exactly once here.
    unsafe {
        CloseHandle(handle);
    }
    identity
}

/// Reads one coherent image and creation time from an open process handle.
fn query_identity(handle: HANDLE) -> Option<ProcessIdentity> {
    let image = query_image_path(handle)?;
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = creation;
    let mut kernel = creation;
    let mut user = creation;
    // SAFETY: all pointers name initialized writable FILETIME values, and
    // `handle` remains open for the complete query.
    let ok = unsafe {
        GetProcessTimes(
            handle,
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    };
    if ok == 0 {
        return None;
    }
    let started = u128::from(creation.dwHighDateTime) << 32 | u128::from(creation.dwLowDateTime);
    Some(ProcessIdentity::new(image, started))
}

/// Reads the image path from an open process handle.
fn query_image_path(handle: HANDLE) -> Option<PathBuf> {
    // The long-path ceiling: QueryFullProcessImageNameW fails rather than
    // truncates when the buffer is too small, so one max-sized buffer
    // needs no grow loop.
    const BUFFER_LEN: u32 = 32768;
    let mut buffer = vec![0u16; BUFFER_LEN as usize];
    let mut size = BUFFER_LEN;
    // SAFETY: `buffer` is valid for `size` UTF-16 code units and `size`
    // starts at its length; on success the call writes the path length
    // back into `size`, never exceeding the buffer.
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &raw mut size) };
    if ok == 0 {
        return None;
    }
    let length = usize::try_from(size).ok()?;
    Some(PathBuf::from(OsString::from_wide(buffer.get(..length)?)))
}
