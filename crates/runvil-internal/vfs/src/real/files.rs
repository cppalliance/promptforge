//! Real-file helpers: the failure-atomic write, ancestor creation,
//! the glob walk, the UTF-8 check of real names, and the metadata
//! mapping.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{map_io, platform};
use crate::error::VfsError;
use crate::path::VfsPath;
use crate::stat::Stat;

/// Uniquifies failure-atomic temp file names within the process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `contents` to `dest`, the real path of `path`,
/// failure-atomically: a sibling temp file plus rename, so a failed
/// write leaves the destination unchanged and no temp file behind.
pub(super) fn atomic_write(dest: &Path, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
    let Some(parent) = dest.parent() else {
        // A destination without a parent names only the filesystem root:
        // the write addresses nothing below the root. No canonical
        // virtual path produces this, so it is a backend impossibility,
        // not a path rule a caller broke; reporting a path reason here
        // would blame the path for the backend's own shape.
        return Err(VfsError::Backend {
            message: format!("write destination of {path} has no parent directory"),
        });
    };
    let Some(name) = dest.file_name() else {
        return Err(VfsError::Backend {
            message: format!("write destination of {path} has no file name"),
        });
    };
    let mut temp_name = OsString::from(".");
    temp_name.push(name);
    temp_name.push(format!(
        ".vfs-tmp-{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let temp = parent.join(temp_name);
    let failed = |err: std::io::Error| map_io(path.as_str(), &err);
    let outcome = (|| {
        let mut file = File::create(&temp).map_err(failed)?;
        file.write_all(contents).map_err(failed)?;
        file.sync_all().map_err(failed)?;
        drop(file);
        fs::rename(&temp, dest).map_err(failed)
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome
}

/// Creates the destination's ancestor directories, matching the memory
/// backend's materialize-on-write semantics.
pub(super) fn create_parent(real: &Path, path: &VfsPath) -> Result<(), VfsError> {
    if let Some(parent) = real.parent() {
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

/// `name`, a real file or directory name that `holder`, a virtual
/// directory, holds, as UTF-8 text. A name that is not UTF-8 has no
/// virtual spelling, so it is refused with the byte offset where it
/// stops being UTF-8.
pub(super) fn utf8_name<'a>(name: &'a OsStr, holder: &str) -> Result<&'a str, VfsError> {
    std::str::from_utf8(name.as_encoded_bytes()).map_err(|err| VfsError::NotUtf8 {
        path: holder.to_owned(),
        valid_up_to: err.valid_up_to(),
    })
}

/// Collects the virtual path of every entry under `dir`, the real
/// directory `virtual_dir` names, files and directories alike, without
/// descending into links: a linked directory is collected, not
/// followed, so the walk can neither leave the mounted root nor loop.
pub(super) fn walk(dir: &Path, virtual_dir: &str, found: &mut Vec<String>) -> Result<(), VfsError> {
    let failed = |err: std::io::Error| map_io(virtual_dir, &err);
    for entry in fs::read_dir(dir).map_err(failed)? {
        let entry = entry.map_err(failed)?;
        let file_name = entry.file_name();
        let name = utf8_name(&file_name, virtual_dir)?;
        let child = if virtual_dir == "/" {
            format!("/{name}")
        } else {
            format!("{virtual_dir}/{name}")
        };
        if entry.file_type().map_err(failed)?.is_dir() {
            walk(&entry.path(), &child, found)?;
        }
        found.push(child);
    }
    Ok(())
}

/// Builds metadata from only the fields the operating system tracks.
pub(super) fn stat_of(metadata: &fs::Metadata) -> Stat {
    Stat::new(platform::file_type_of(metadata.file_type()), metadata.len())
        .with_mode(platform::mode_of(metadata))
        .with_modified(metadata.modified().ok())
        .with_created(metadata.created().ok())
}
