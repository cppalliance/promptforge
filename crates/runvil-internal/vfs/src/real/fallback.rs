//! The real backend's arms on a platform that is neither Unix nor
//! Windows: an identity path is the real path, only files, directories,
//! and links are told apart, no mode bits are reported, and case is
//! kept.

use std::fs;
use std::path::PathBuf;

use crate::stat::FileType;

/// Whether the file system treats paths that differ only in case as one
/// file: assumed not, as POSIX file systems do.
pub(super) const FOLDS_CASE: bool = false;

/// Translates an identity-mode virtual path to a real path.
pub(super) fn identity_to_real(virtual_path: &str) -> PathBuf {
    PathBuf::from(virtual_path)
}

/// Maps a real file's type to the named POSIX kinds that `std` tells
/// apart on every platform.
pub(super) fn file_type_of(file_type: fs::FileType) -> FileType {
    if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else {
        FileType::File
    }
}

/// Whether the entry is a directory link needing `remove_dir`: never,
/// as `remove_file` removes any link.
pub(super) fn is_dir_link(_: &fs::Metadata) -> bool {
    false
}

/// The POSIX mode bits, which `std` does not expose here.
pub(super) fn mode_of(_: &fs::Metadata) -> Option<u32> {
    None
}
