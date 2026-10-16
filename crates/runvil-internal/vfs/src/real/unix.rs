//! The Unix arms of the real backend: an identity path is the real
//! path, every POSIX file kind and the mode bits are reported, and only
//! macOS folds case.

use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;

use crate::stat::FileType;

/// Whether the file system treats paths that differ only in case as one
/// file: macOS's default volumes do, other Unix file systems do not.
pub(super) const FOLDS_CASE: bool = cfg!(target_os = "macos");

/// Translates an identity-mode virtual path to a real path.
pub(super) fn identity_to_real(virtual_path: &str) -> PathBuf {
    PathBuf::from(virtual_path)
}

/// Maps a real file's type to the named POSIX kinds.
pub(super) fn file_type_of(file_type: fs::FileType) -> FileType {
    if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else if file_type.is_file() {
        FileType::File
    } else if file_type.is_fifo() {
        FileType::Fifo
    } else if file_type.is_socket() {
        FileType::Socket
    } else if file_type.is_char_device() {
        FileType::CharDevice
    } else if file_type.is_block_device() {
        FileType::BlockDevice
    } else {
        FileType::File
    }
}

/// Whether the entry is a directory link needing `remove_dir`: never on
/// Unix, where `remove_file` removes any link.
pub(super) fn is_dir_link(_: &fs::Metadata) -> bool {
    false
}

/// The POSIX mode bits, which Unix always tracks.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the other platforms return None; the Option unifies the platform signatures"
)]
pub(super) fn mode_of(metadata: &fs::Metadata) -> Option<u32> {
    Some(metadata.permissions().mode())
}
