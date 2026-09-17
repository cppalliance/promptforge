//! Confined workspace filesystem access: directory trees, file reads, and
//! file writes jailed to roots explicitly granted through drag and drop.
//!
//! A dropped folder becomes a granted root; a dropped file grants its parent
//! directory. The in-memory grant set is the confinement source of truth;
//! an optional workspace file (the `backing` module) mirrors it between
//! sessions and is never consulted on a request path. Every request path is
//! checked lexically (no `..`, and on Windows no NTFS alternate data
//! stream names) and then
//! canonicalized and prefix-matched against the canonical grants before any
//! filesystem operation, so traversal, symlink escapes, and UNC aliases
//! cannot reach outside a grant. This is the same jail shape as the
//! gateway's artifact-cache `confine.rs`, with canonicalization performing
//! the resolution that module's component walk performs by hand.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::error::WorkspaceError;
use crate::workspace_file::now_rfc3339;

#[path = "workspace-backing.rs"]
mod backing;
#[path = "workspace-pointer.rs"]
mod pointer;
#[path = "workspace-token.rs"]
mod token;

use backing::Backing;
pub use backing::{GrantEntry, WorkspaceSummary};
use token::{current_token, file_token};
#[cfg(test)]
use token::{hash_token, mtime_token};

/// The largest file the workspace reads or accepts for a write: the editor
/// targets source text, not media, so one MiB is generous.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Whether a tree entry is a directory or a regular file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// A directory.
    Directory,
    /// A regular file.
    File,
}

/// One entry in a directory listing.
#[derive(Debug, Serialize)]
pub struct TreeEntry {
    /// The entry's file name (lossy for non-Unicode names).
    name: String,
    /// The entry's full path, ready to pass back to the API.
    path: PathBuf,
    /// Directory or file.
    kind: EntryKind,
    /// Byte length (0 for directories).
    size: u64,
    /// Modification time in milliseconds since the Unix epoch.
    modified_ms: u64,
    /// Whether the entry is currently on disk. Directory listings only
    /// enumerate what exists, so their entries are always `true`; a
    /// granted root deleted from disk lists as `false` so the panel can
    /// flag it for cleanup.
    exists: bool,
}

/// One level of a workspace directory tree.
#[derive(Debug, Serialize)]
pub struct TreeListing {
    /// The listed directory; `None` when the listing is the granted roots.
    path: Option<PathBuf>,
    /// Directories before files, each group ordered by name.
    entries: Vec<TreeEntry>,
}

/// A file's text plus the metadata a writer needs to detect conflicts.
#[derive(Debug, Serialize)]
pub struct FileContents {
    /// The canonical file path.
    path: PathBuf,
    /// Byte length.
    size: u64,
    /// The opaque conflict token a writer must echo back as
    /// `expected_token`; see [`file_token`] for its derivation.
    token: String,
    /// The file's UTF-8 text.
    text: String,
}

/// What memory keeps beside each granted root so the file holds the
/// workspace's true history: the grant's order among its peers and when
/// it was made. Confinement never reads it. Its two readers are in the
/// backing module: the grant mirror in [`Workspace::grant_and_persist`],
/// which sends a new grant's row to the open file, and the save-as row
/// builder, which writes every grant into a new file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantMeta {
    /// Stable tree order: the file's `position` when the grant was loaded
    /// from one, one past the current maximum when granted in session.
    pub(crate) position: u32,
    /// RFC 3339 grant time.
    pub(crate) added_at: String,
}

/// The workspace: the granted roots and the optional backing file.
///
/// Cloning shares the same state, so the router state and every handler
/// see grants registered through `POST /workspace/grant` immediately. The
/// grant set is the confinement source of truth; the backing file, when
/// present, is its persistent mirror and is swapped at runtime by open,
/// save-as, and duplicate, each recorded in the last-workspace pointer.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    /// The granted roots, in canonical form, each with its grant order
    /// and time. Read-hot: its own lock.
    grants: Arc<RwLock<BTreeMap<PathBuf, GrantMeta>>>,
    /// The backing workspace file; `None` while the workspace is
    /// ephemeral.
    backing: Arc<RwLock<Option<Backing>>>,
    /// Serializes ui-state puts so values reach the backing file's actor
    /// in the order they reached memory; see [`Workspace::put_ui_state`].
    ui_state_puts: Arc<tokio::sync::Mutex<()>>,
    /// Where the last-used file is remembered between runs; `None` when
    /// built without a state directory (see [`Workspace::with_state_dir`]).
    pointer: Option<pointer::LastWorkspacePointer>,
}

impl Workspace {
    /// Creates an ephemeral workspace with no grants and no file.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `path` as a granted root in memory only: a directory
    /// grants itself, a file grants its parent directory. A new grant
    /// takes the position one past the current maximum and the current
    /// time; a path already granted keeps the order and time it has.
    /// Handlers use [`Workspace::grant_and_persist`], which also mirrors
    /// the grant into the backing file.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::ForbiddenComponent`] when the path carries
    /// a `..` or stream name, [`WorkspaceError::ResolveGrant`] when it
    /// cannot be canonicalized, and [`WorkspaceError::NotFound`] when a
    /// file path has no parent directory.
    pub fn grant(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        self.grant_with_meta(path).map(|(root, _)| root)
    }

    /// [`Workspace::grant`], also returning the order and time memory
    /// holds for the root afterwards: the meta just assigned to a new
    /// grant, or the one an already-granted path keeps. Read under the
    /// same write lock as the insert, so the caller sees the row memory
    /// holds rather than a later snapshot.
    fn grant_with_meta(&self, path: &Path) -> Result<(PathBuf, GrantMeta), WorkspaceError> {
        reject_forbidden(path)?;
        let canonical = canonicalize_simplified(path)
            .map_err(|source| WorkspaceError::ResolveGrant { source })?;
        let root = if canonical.is_dir() {
            canonical
        } else {
            canonical
                .parent()
                .map(Path::to_owned)
                .ok_or(WorkspaceError::NotFound)?
        };
        let mut grants = self.grants.write().unwrap_or_else(PoisonError::into_inner);
        let position = grants
            .values()
            .map(|meta| meta.position)
            .max()
            .map_or(0, |max| max.saturating_add(1));
        let meta = grants
            .entry(root.clone())
            .or_insert_with(|| GrantMeta {
                position,
                added_at: now_rfc3339(),
            })
            .clone();
        Ok((root, meta))
    }

    /// Removes `path` from the granted roots in memory only, by exact
    /// canonical match; handlers use [`Workspace::revoke_and_persist`].
    /// A root deleted from disk stays revocable by the literal stored
    /// key. Nested grants are independent: revoking a parent leaves a
    /// separately granted child intact, and files under the child stay
    /// reachable while everything else under the parent loses access on
    /// its next operation.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::ForbiddenComponent`] when the path carries
    /// a `..` or stream name, [`WorkspaceError::ResolveGrant`] when
    /// canonicalization fails for a reason other than absence, and
    /// [`WorkspaceError::NotGranted`] when the resolved path is not a
    /// granted root.
    pub fn revoke(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        reject_forbidden(path)?;
        // A root deleted from disk no longer canonicalizes, but its grant
        // must stay removable: fall back to the literal path, which matches
        // the stored canonical key the roots listing handed the client.
        let canonical = match canonicalize_simplified(path) {
            Ok(canonical) => canonical,
            Err(source) if source.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
            Err(source) => return Err(WorkspaceError::ResolveGrant { source }),
        };
        let removed = self
            .grants
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&canonical)
            .is_some();
        if removed {
            Ok(canonical)
        } else {
            Err(WorkspaceError::NotGranted)
        }
    }

    /// The granted roots in canonical (path) order. Grant order is kept
    /// beside each root and reaches the file through save-as, not here.
    #[must_use]
    pub fn granted_roots(&self) -> Vec<PathBuf> {
        self.grants
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// Lists one level of `path`, or the granted roots when `path` is
    /// `None` or empty. Directories sort before files, each group ordered
    /// by name.
    ///
    /// # Errors
    /// Returns [`WorkspaceError`] when the path is forbidden, outside every
    /// grant, missing, not a directory, or cannot be listed.
    pub fn tree(&self, path: Option<&Path>) -> Result<TreeListing, WorkspaceError> {
        match path {
            None => Ok(self.grants_listing()),
            Some(path) if path.as_os_str().is_empty() => Ok(self.grants_listing()),
            Some(path) => self.directory_listing(path),
        }
    }

    /// Reads a confined UTF-8 text file with its size and conflict
    /// token. Binary and oversized files are rejected.
    ///
    /// # Errors
    /// Returns [`WorkspaceError`] when the path is forbidden, outside every
    /// grant, missing, not a regular file, binary, not UTF-8, oversized, or
    /// cannot be read.
    pub fn read_file(&self, path: &Path) -> Result<FileContents, WorkspaceError> {
        let canonical = self.confine_existing(path)?;
        let metadata =
            fs::metadata(&canonical).map_err(|source| WorkspaceError::InspectPath { source })?;
        if !metadata.is_file() {
            return Err(WorkspaceError::NotAFile);
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(WorkspaceError::FileTooLarge {
                limit: MAX_FILE_BYTES,
            });
        }
        let bytes = fs::read(&canonical).map_err(|source| WorkspaceError::ReadFile { source })?;
        if bytes.contains(&0) {
            return Err(WorkspaceError::BinaryFile);
        }
        let token = file_token(&metadata, &bytes);
        let text = String::from_utf8(bytes).map_err(|_| WorkspaceError::NotUtf8)?;
        Ok(FileContents {
            path: canonical,
            size: metadata.len(),
            token,
            text,
        })
    }

    /// Writes `text` to a confined path, creating the file when it does not
    /// exist. When the file exists, `expected_token` must match its current
    /// conflict token or the write is refused as a conflict.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::FileTooLarge`] when the text exceeds the
    /// size limit, [`WorkspaceError::ModifiedConflict`] when the token is
    /// stale, absent, or underivable for the existing file, and otherwise
    /// [`WorkspaceError`] when the path is forbidden, outside every grant,
    /// not a regular file, or cannot be written.
    pub fn write_file(
        &self,
        path: &Path,
        text: &str,
        expected_token: Option<&str>,
    ) -> Result<FileContents, WorkspaceError> {
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(WorkspaceError::FileTooLarge {
                limit: MAX_FILE_BYTES,
            });
        }
        let canonical = self.confine_for_write(path)?;
        match fs::metadata(&canonical) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    return Err(WorkspaceError::NotAFile);
                }
                // Fail closed: only a derivable on-disk token that equals
                // the writer's token proves the file is unchanged.
                match (current_token(&canonical, &metadata), expected_token) {
                    (Some(current), Some(expected)) if current == expected => {}
                    _ => return Err(WorkspaceError::ModifiedConflict),
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(WorkspaceError::InspectPath { source }),
        }
        workshop_support::write_atomic(&canonical, text.as_bytes())
            .map_err(|source| WorkspaceError::WriteFile { source })?;
        let metadata =
            fs::metadata(&canonical).map_err(|source| WorkspaceError::InspectPath { source })?;
        Ok(FileContents {
            path: canonical,
            size: metadata.len(),
            token: file_token(&metadata, text.as_bytes()),
            text: text.to_owned(),
        })
    }

    /// The granted roots rendered as a synthetic directory listing.
    fn grants_listing(&self) -> TreeListing {
        let entries = self
            .granted_roots()
            .into_iter()
            .map(|root| {
                let metadata = fs::metadata(&root).ok();
                // The folder's own name reads better than the full path in
                // the tree; the path stays available as the row tooltip. A
                // drive root (C:\) has no file name and shows the path.
                let name = root.file_name().map_or_else(
                    || root.to_string_lossy().into_owned(),
                    |name| name.to_string_lossy().into_owned(),
                );
                TreeEntry {
                    name,
                    path: root,
                    kind: EntryKind::Directory,
                    size: 0,
                    modified_ms: metadata.as_ref().map_or(0, modified_ms),
                    exists: metadata.is_some(),
                }
            })
            .collect();
        TreeListing {
            path: None,
            entries,
        }
    }

    /// Lists one level of an existing confined directory.
    fn directory_listing(&self, path: &Path) -> Result<TreeListing, WorkspaceError> {
        let canonical = self.confine_existing(path)?;
        let metadata =
            fs::metadata(&canonical).map_err(|source| WorkspaceError::InspectPath { source })?;
        if !metadata.is_dir() {
            return Err(WorkspaceError::NotADirectory);
        }
        let mut entries = Vec::new();
        for entry in
            fs::read_dir(&canonical).map_err(|source| WorkspaceError::ListDirectory { source })?
        {
            let entry = entry.map_err(|source| WorkspaceError::ListDirectory { source })?;
            let metadata = entry
                .metadata()
                .map_err(|source| WorkspaceError::InspectPath { source })?;
            let kind = if metadata.is_dir() {
                EntryKind::Directory
            } else {
                EntryKind::File
            };
            entries.push(TreeEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                kind,
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                modified_ms: modified_ms(&metadata),
                exists: true,
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != EntryKind::Directory)
                .cmp(&(b.kind != EntryKind::Directory))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(TreeListing {
            path: Some(canonical),
            entries,
        })
    }

    /// Canonicalizes an existing path and confines it to the grants.
    fn confine_existing(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
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
    fn confine_for_write(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
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
fn canonicalize_simplified(path: &Path) -> io::Result<PathBuf> {
    Ok(dunce::simplified(&path.canonicalize()?).to_path_buf())
}

/// Rejects the lexical tricks canonicalization would otherwise hide: `..`
/// traversal everywhere, and `:` alternate data stream names on Windows,
/// where a colon in a name addresses an NTFS stream. Elsewhere a colon is
/// an ordinary filename character and passes.
fn reject_forbidden(path: &Path) -> Result<(), WorkspaceError> {
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
fn modified_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
#[path = "workspace-tests.rs"]
mod tests;
#[cfg(test)]
#[path = "workspace-tests-close.rs"]
mod tests_close;
#[cfg(test)]
#[path = "workspace-tests-reopen.rs"]
mod tests_reopen;
