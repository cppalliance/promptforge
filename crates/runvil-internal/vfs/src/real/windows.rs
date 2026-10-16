//! The Windows arms of the real backend: an identity path spells its
//! drive as a leading segment, `std` tells apart only files,
//! directories, and links, no mode bits exist, and case folds.

use std::fs;
use std::os::windows::fs::FileTypeExt;
use std::path::PathBuf;

use crate::stat::FileType;

/// Whether the file system treats paths that differ only in case as one
/// file: NTFS and FAT do.
pub(super) const FOLDS_CASE: bool = true;

/// Translates an identity-mode virtual path to a real path. The virtual
/// spelling of `C:\Users\x` is `/C:/Users/x`: a leading slash before a
/// drive letter is stripped.
pub(super) fn identity_to_real(virtual_path: &str) -> PathBuf {
    let bytes = virtual_path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return PathBuf::from(&virtual_path[1..]);
    }
    PathBuf::from(virtual_path)
}

/// Maps a real file's type to the named POSIX kinds. Windows distinguishes
/// only files, directories, and symlinks through `std`.
pub(super) fn file_type_of(file_type: fs::FileType) -> FileType {
    if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else {
        FileType::File
    }
}

/// Whether the entry is a directory link, a junction included, which
/// Windows removes with `remove_dir`: `remove_file` fails on one.
pub(super) fn is_dir_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink_dir()
}

/// The POSIX mode bits, which Windows does not track.
pub(super) fn mode_of(_: &fs::Metadata) -> Option<u32> {
    None
}
