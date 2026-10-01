//! Path resolution: the identity spelling, the rooted join, and the
//! canonicalize containment that keeps a rooted path under its root.

use std::fs;
use std::path::{Path, PathBuf};

use super::map_io;
use crate::error::VfsError;
use crate::path::VfsPath;

/// How virtual paths reach real paths: verbatim, or contained under a
/// canonicalized root.
#[derive(Debug, Clone)]
pub(super) enum HostRoot {
    /// The virtual path is the real path (modulo the Windows drive
    /// letter spelling).
    Identity,
    /// Chroot-style: the virtual root is this canonical real directory.
    Rooted(PathBuf),
}

/// Translates an identity-mode virtual path to a real path. On Windows
/// the virtual spelling of `C:\Users\x` is `/C:/Users/x`: a leading
/// slash before a drive letter is stripped.
#[cfg(windows)]
pub(super) fn identity_to_host(virtual_path: &str) -> PathBuf {
    let bytes = virtual_path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return PathBuf::from(&virtual_path[1..]);
    }
    PathBuf::from(virtual_path)
}

/// Translates an identity-mode virtual path to a real path.
#[cfg(not(windows))]
pub(super) fn identity_to_host(virtual_path: &str) -> PathBuf {
    PathBuf::from(virtual_path)
}

/// Translates a real path back to its identity-mode virtual spelling:
/// forward slashes, and on Windows a leading slash before a drive
/// letter (`C:\Users\x` becomes `/C:/Users/x`).
#[cfg(windows)]
pub(super) fn identity_to_virtual(host: &Path) -> String {
    let spelled = host.to_string_lossy().replace('\\', "/");
    let bytes = spelled.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return format!("/{spelled}");
    }
    spelled
}

/// Translates a real path back to its identity-mode virtual spelling.
#[cfg(not(windows))]
pub(super) fn identity_to_virtual(host: &Path) -> String {
    host.to_string_lossy().into_owned()
}

/// Joins a canonical virtual path onto a real directory. The virtual path is
/// canonical (dot segments resolved at receipt, forward slashes), so
/// the join cannot escape lexically.
pub(super) fn join_virtual(root: &Path, virtual_path: &str) -> PathBuf {
    let mut host = root.to_path_buf();
    for segment in virtual_path.split('/').filter(|s| !s.is_empty()) {
        host.push(segment);
    }
    host
}

/// Canonicalize containment: the candidate's nearest existing ancestor
/// is canonicalized and must sit under the (already canonical) root;
/// the missing tail is re-appended lexically. This catches link escapes
/// for existing paths while still resolving paths yet to be created.
/// A dangling link on the way up is refused: its target cannot be
/// canonicalized, and the operating system would follow it when creating.
pub(super) fn contain(
    root: &Path,
    candidate: &Path,
    original: &VfsPath,
) -> Result<PathBuf, VfsError> {
    let denied = || VfsError::PermissionDenied {
        path: original.to_string(),
        reason: format!("{original} escapes the mounted root"),
    };
    let mut ancestor = candidate;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if ancestor.exists() {
            let canonical =
                fs::canonicalize(ancestor).map_err(|err| map_io(original.as_str(), &err))?;
            if !canonical.starts_with(root) {
                return Err(denied());
            }
            let mut resolved = canonical;
            for component in tail.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }
        if ancestor.symlink_metadata().is_ok() {
            return Err(VfsError::PermissionDenied {
                path: original.to_string(),
                reason: format!("{original} passes through a dangling symbolic link"),
            });
        }
        let Some(parent) = ancestor.parent() else {
            return Err(denied());
        };
        let Some(name) = ancestor.file_name() else {
            return Err(denied());
        };
        tail.push(name);
        ancestor = parent;
    }
}

/// No-follow containment: the candidate's parent is contained as in
/// [`contain`], so its nearest existing ancestor must sit under the
/// root, and the final component is appended unchanged, so a
/// final-component link is addressed as a link. The root itself
/// resolves to the root.
pub(super) fn contain_no_follow(
    root: &Path,
    candidate: &Path,
    original: &VfsPath,
) -> Result<PathBuf, VfsError> {
    if candidate == root {
        return Ok(root.to_path_buf());
    }
    let (Some(parent), Some(name)) = (candidate.parent(), candidate.file_name()) else {
        return Err(VfsError::PermissionDenied {
            path: original.to_string(),
            reason: format!("{original} escapes the mounted root"),
        });
    };
    let mut resolved = contain(root, parent, original)?;
    resolved.push(name);
    Ok(resolved)
}
