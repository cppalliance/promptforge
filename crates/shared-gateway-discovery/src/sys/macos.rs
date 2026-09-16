//! macOS process identity lookup via libproc: `proc_pidpath` supplies the
//! image and `PROC_PIDTBSDINFO` supplies the process start timeval.

use std::ffi::OsString;
use std::mem::{MaybeUninit, size_of};
use std::os::unix::ffi::OsStringExt as _;
use std::path::PathBuf;

use super::ProcessIdentity;

/// Buffer size for `proc_pidpath`: `PROC_PIDPATHINFO_MAXSIZE` from
/// `libproc.h` (4 * MAXPATHLEN).
const PROC_PIDPATHINFO_MAXSIZE: u32 = 4096;
/// `PROC_PIDTBSDINFO` from `libproc.h`.
const PROC_PIDTBSDINFO: i32 = 3;

/// The stable prefix and start fields of Darwin's `proc_bsdinfo`.
#[repr(C)]
struct ProcBsdInfo {
    pbi_flags: u32,
    pbi_status: u32,
    pbi_xstatus: u32,
    pbi_pid: u32,
    pbi_ppid: u32,
    pbi_uid: u32,
    pbi_gid: u32,
    pbi_ruid: u32,
    pbi_rgid: u32,
    pbi_svuid: u32,
    pbi_svgid: u32,
    rfu_1: u32,
    pbi_comm: [libc::c_char; 16],
    pbi_name: [libc::c_char; 32],
    pbi_nfiles: u32,
    pbi_pgid: u32,
    pbi_pjobc: u32,
    e_tdev: u32,
    e_tpgid: u32,
    pbi_nice: i32,
    pbi_start_tvsec: u64,
    pbi_start_tvusec: u64,
}

/// The image and start timeval for process `pid`, or `None` when the
/// process disappears, changes identity during observation, or refuses
/// either query.
pub(crate) fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    let first_start = process_start(pid)?;
    let image = process_image_path(pid)?;
    let second_start = process_start(pid)?;
    (first_start == second_start).then(|| ProcessIdentity::new(image, first_start))
}

/// Reads the kernel's path for one live process.
fn process_image_path(pid: u32) -> Option<PathBuf> {
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

/// Reads the process start timeval through `PROC_PIDTBSDINFO`.
fn process_start(pid: u32) -> Option<u128> {
    let pid = i32::try_from(pid).ok()?;
    let buffer_size = i32::try_from(size_of::<ProcBsdInfo>()).ok()?;
    let mut info = MaybeUninit::<ProcBsdInfo>::uninit();
    // SAFETY: `info` points to writable storage of exactly `buffer_size`
    // bytes. A full-size success initializes the complete structure before
    // `assume_init`; every other result returns without reading it.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            buffer_size,
        )
    };
    if written != buffer_size {
        return None;
    }
    // SAFETY: the full-size `proc_pidinfo` success above initialized every
    // byte of the `ProcBsdInfo` output structure.
    let info = unsafe { info.assume_init() };
    Some(u128::from(info.pbi_start_tvsec) << 64 | u128::from(info.pbi_start_tvusec))
}
