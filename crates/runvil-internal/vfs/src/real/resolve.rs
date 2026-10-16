//! Path resolution: the rooted join and the canonicalize containment
//! that keeps a rooted path under its root.

use std::fs;
use std::path::{Path, PathBuf};

use super::map_io;
use crate::error::VfsError;
use crate::path::VfsPath;

/// How virtual paths reach real paths: verbatim, or contained under a
/// canonicalized root.
#[derive(Debug, Clone)]
pub(super) enum RealRoot {
    /// The virtual path is the real path (modulo the Windows drive
    /// letter spelling).
    Identity,
    /// Chroot-style: the virtual root is this canonical real directory.
    Rooted(PathBuf),
}

/// Joins a canonical virtual path onto a real directory. The virtual path is
/// canonical (dot segments resolved at receipt, forward slashes), so
/// the join cannot escape lexically.
pub(super) fn join_virtual(root: &Path, virtual_path: &str) -> PathBuf {
    let mut real = root.to_path_buf();
    for segment in virtual_path.split('/').filter(|s| !s.is_empty()) {
        real.push(segment);
    }
    real
}

/// The refusal of `original`, which leaves the root as `detail` says.
fn outside(original: &VfsPath, detail: String) -> VfsError {
    VfsError::OutsideRoot {
        path: original.to_string(),
        detail,
    }
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
    let escapes = || outside(original, format!("{original} escapes the mounted root"));
    let mut ancestor = candidate;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if ancestor.exists() {
            let canonical =
                fs::canonicalize(ancestor).map_err(|err| map_io(original.as_str(), &err))?;
            if !canonical.starts_with(root) {
                return Err(escapes());
            }
            let mut resolved = canonical;
            for component in tail.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }
        if ancestor.symlink_metadata().is_ok() {
            return Err(outside(
                original,
                format!("{original} passes through a dangling symbolic link"),
            ));
        }
        let Some(parent) = ancestor.parent() else {
            return Err(escapes());
        };
        let Some(name) = ancestor.file_name() else {
            return Err(escapes());
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
        return Err(outside(
            original,
            format!("{original} escapes the mounted root"),
        ));
    };
    let mut resolved = contain(root, parent, original)?;
    resolved.push(name);
    Ok(resolved)
}
