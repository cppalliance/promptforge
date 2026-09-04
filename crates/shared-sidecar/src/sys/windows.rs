//! Windows process image lookup: `OpenProcess` +
//! `QueryFullProcessImageNameW` answer liveness and identity together - a
//! dead pid opens no handle once its last handle closes.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt as _;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

/// The full image path of process `pid`, or `None` when the process is
/// dead or refuses a limited query.
pub(crate) fn process_image_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: `OpenProcess` takes a valid access mask and pid; the
    // returned handle is either null (checked) or a live process handle
    // that `CloseHandle` below releases exactly once.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let image = query_image_path(handle);
    // SAFETY: `handle` is the live process handle returned by the
    // `OpenProcess` above, closed exactly once here.
    unsafe {
        CloseHandle(handle);
    }
    image
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
