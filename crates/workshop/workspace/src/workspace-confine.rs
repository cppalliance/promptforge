//! The confinement half of the workspace: the path checks every request
//! passes before a filesystem operation, and the path helpers the grant
//! and listing code share with them.
//!
//! A request path is first rejected lexically (`..` anywhere, and on
//! Windows an NTFS alternate data stream name), then canonicalized so
//! symlinks, `..`-free aliases, and UNC forms resolve to one on-disk path,
//! and finally prefix-matched against the canonical grants. Write targets
//! that do not exist yet confine their canonical parent and reattach the
//! file name, so a new file can only ever land inside a grant.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::PoisonError;
use std::time::UNIX_EPOCH;

use crate::error::WorkspaceError;

use super::Workspace;

impl Workspace {
    /// Canonicalizes an existing path and confines it to the grants.
    pub(super) fn confine_existing(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        reject_forbidden(path)?;
        let canonical = canonicalize_simplified(path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                WorkspaceError::NotFound
            } else {
                WorkspaceError::ResolvePath { source }
            }
        })?;
        self.check_confined(canonical)
    }

    /// Confines a write target: an existing path canonicalizes directly; a
    /// new file confines its canonicalized parent and reattaches its name.
    pub(super) fn confine_for_write(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        reject_forbidden(path)?;
        match canonicalize_simplified(path) {
            Ok(canonical) => self.check_confined(canonical),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                // A dangling symlink canonicalizes as NotFound, but fs::write
                // would follow it and create the target outside the grant.
                match fs::symlink_metadata(path) {
                    Ok(_) => return Err(WorkspaceError::OutsideGrants),
                    Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                    Err(source) => return Err(WorkspaceError::InspectPath { source }),
                }
                let parent = path.parent().ok_or(WorkspaceError::NotFound)?;
                let canonical_parent = canonicalize_simplified(parent).map_err(|source| {
                    if source.kind() == io::ErrorKind::NotFound {
                        WorkspaceError::NotFound
                    } else {
                        WorkspaceError::ResolvePath { source }
                    }
                })?;
                let name = path.file_name().ok_or(WorkspaceError::ForbiddenComponent)?;
                self.check_confined(canonical_parent.join(name))
            }
            Err(source) => Err(WorkspaceError::ResolvePath { source }),
        }
    }

    /// Admits a canonical path that starts with a granted root.
    fn check_confined(&self, canonical: PathBuf) -> Result<PathBuf, WorkspaceError> {
        let grants = self.grants.read().unwrap_or_else(PoisonError::into_inner);
        if grants.keys().any(|root| canonical.starts_with(root)) {
            Ok(canonical)
        } else {
            Err(WorkspaceError::OutsideGrants)
        }
    }
}

/// Canonicalizes and strips Windows' `\\?\` verbatim prefix (a no-op on
/// other platforms). Every path the workspace stores, compares, or returns
/// goes through here, so grants and confinement checks stay in one form
/// and the UI never sees the prefix.
pub(super) fn canonicalize_simplified(path: &Path) -> io::Result<PathBuf> {
    Ok(dunce::simplified(&path.canonicalize()?).to_path_buf())
}

/// Rejects the lexical tricks canonicalization would otherwise hide: `..`
/// traversal everywhere, and `:` alternate data stream names on Windows,
/// where a colon in a name addresses an NTFS stream. Elsewhere a colon is
/// an ordinary filename character and passes.
pub(super) fn reject_forbidden(path: &Path) -> Result<(), WorkspaceError> {
    for component in path.components() {
        match component {
            Component::ParentDir => return Err(WorkspaceError::ForbiddenComponent),
            #[cfg(windows)]
            Component::Normal(name) if name.to_string_lossy().contains(':') => {
                return Err(WorkspaceError::ForbiddenComponent);
            }
            _ => {}
        }
    }
    Ok(())
}

/// A file's modification time as milliseconds since the Unix epoch.
pub(super) fn modified_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
