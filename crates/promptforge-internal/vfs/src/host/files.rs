//! Host file helpers: the failure-atomic write, ancestor creation,
//! the glob walk, and the metadata mapping.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::map_io;
use crate::error::VfsError;
use crate::path::VfsPath;
use crate::stat::{FileType, Stat};

/// Uniquifies failure-atomic temp file names within the process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `contents` to `dest` failure-atomically: a sibling temp file
/// plus rename, so a failed write leaves the destination unchanged and
/// no temp file behind.
pub(super) fn atomic_write(dest: &Path, contents: &[u8]) -> Result<(), VfsError> {
    let display = dest.to_string_lossy().into_owned();
    let Some(parent) = dest.parent() else {
        // A destination without a parent names only the filesystem root:
        // the write addresses nothing below the root. No canonical
        // virtual path produces this, so it is a backend impossibility,
        // not a path rule a caller broke; reporting a path reason here
        // would blame the path for the backend's own shape.
        return Err(VfsError::Backend {
            message: format!("write destination has no parent directory: {display:?}"),
        });
    };
    let Some(name) = dest.file_name() else {
        return Err(VfsError::Backend {
            message: format!("write destination has no file name: {display:?}"),
        });
    };
    let temp = parent.join(format!(
        ".{}.vfs-tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = (|| {
        let mut file = File::create(&temp).map_err(|err| map_io(&display, &err))?;
        file.write_all(contents)
            .map_err(|err| map_io(&display, &err))?;
        file.sync_all().map_err(|err| map_io(&display, &err))?;
        drop(file);
        fs::rename(&temp, dest).map_err(|err| map_io(&display, &err))
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome
}

/// Creates the destination's ancestor directories, matching the memory
/// backend's materialize-on-write semantics.
pub(super) fn create_parent(host: &Path, path: &VfsPath) -> Result<(), VfsError> {
    if let Some(parent) = host.parent() {
        fs::create_dir_all(parent).map_err(|err| map_io(path.as_str(), &err))?;
    }
    Ok(())
}

/// The directory the walk must start from: the literal leading
/// segments of the pattern. A wildcard-free pattern can match only the
/// literal path itself, so the walk starts from its parent directory
/// for the literal file to be among the collected candidates, matching
/// the memory backend's parity.
pub(super) fn walk_root(pattern: &str) -> &str {
    let literal = match pattern.find('*') {
        Some(star) => &pattern[..star],
        None => pattern,
    };
    match literal.rfind('/') {
        Some(0) | None => "/",
        Some(slash) => &literal[..slash],
    }
}

/// Collects every path under `dir`, files and directories, without
/// descending into links: a linked directory is collected, not
/// followed, so the walk can neither leave the mounted root nor loop.
pub(super) fn walk(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), VfsError> {
    let entries = fs::read_dir(dir).map_err(|err| map_io(&dir.to_string_lossy(), &err))?;
    for entry in entries {
        let entry = entry.map_err(|err| map_io(&dir.to_string_lossy(), &err))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| map_io(&dir.to_string_lossy(), &err))?;
        found.push(path.clone());
        if file_type.is_dir() {
            walk(&path, found)?;
        }
    }
    Ok(())
}

/// Maps a host file type to the named POSIX kinds.
#[cfg(unix)]
fn file_type_of(file_type: fs::FileType) -> FileType {
    use std::os::unix::fs::FileTypeExt;
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

/// Maps a host file type to the named POSIX kinds. Windows distinguishes
/// only files, directories, and symlinks through `std`.
#[cfg(not(unix))]
fn file_type_of(file_type: fs::FileType) -> FileType {
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
#[cfg(windows)]
pub(super) fn is_dir_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::FileTypeExt;
    metadata.file_type().is_symlink_dir()
}

/// Whether the entry is a directory link needing `remove_dir`: never
/// off Windows, where `remove_file` removes any link.
#[cfg(not(windows))]
pub(super) fn is_dir_link(_: &fs::Metadata) -> bool {
    false
}

/// POSIX mode bits where the host tracks them.
#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the not(unix) variant returns None; the Option unifies the platform signatures"
)]
fn mode_of(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

/// POSIX mode bits where the host tracks them: not on Windows.
#[cfg(not(unix))]
fn mode_of(_: &fs::Metadata) -> Option<u32> {
    None
}

/// Builds metadata without fabricating fields the host does not track.
pub(super) fn stat_of(metadata: &fs::Metadata) -> Stat {
    Stat {
        file_type: file_type_of(metadata.file_type()),
        size: metadata.len(),
        mode: mode_of(metadata),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
    }
}
