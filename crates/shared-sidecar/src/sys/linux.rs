//! Linux process image lookup: the `/proc/<pid>/exe` symlink answers both
//! liveness and identity - a dead process (or a zombie) has no `exe` link
//! to read.

use std::path::PathBuf;

/// The kernel's path for the process's executable, or `None` when the
/// process is gone or the link cannot be read (a dead pid, a zombie, or
/// an unreadable `/proc`).
pub(crate) fn process_image_path(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}
