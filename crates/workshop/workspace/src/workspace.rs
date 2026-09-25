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
//! cannot reach outside a grant. This is the same confinement as the
//! gateway's artifact-cache `confine.rs`, with canonicalization performing
//! the resolution that module's component walk performs by hand.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, PoisonError, RwLock};

use serde::Serialize;

use crate::error::WorkspaceError;
use crate::workspace_file::{WindowState, now_rfc3339};

mod backing;
mod confine;
mod pointer;
mod token;

use backing::Backing;
use confine::{canonicalize_simplified, modified_ms, reject_forbidden};
use token::{current_token, file_token};
#[cfg(test)]
use token::{hash_token, mtime_token};

/// The largest file the workspace reads or accepts for a write: the editor
/// targets source text, so one MiB is generous.
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

/// One granted root as the workspace reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantEntry {
    /// The canonical granted root.
    pub path: PathBuf,
    /// Whether the root is on disk right now. A vanished root stays
    /// granted and listed so the user can see it and revoke it.
    pub exists: bool,
}

/// The workspace as a whole: its file, if any, and what it holds; built
/// by [`Workspace::current`] in the backing module.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSummary {
    /// The backing file; `None` while the workspace is ephemeral.
    pub path: Option<PathBuf>,
    /// The display name: the file's own, or `Untitled` while ephemeral.
    pub name: String,
    /// The granted roots in canonical order.
    pub grants: Vec<GrantEntry>,
    /// The saved window geometry; `None` while ephemeral or never saved.
    pub window_state: Option<WindowState>,
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
#[derive(Debug, Clone)]
pub struct Workspace {
    /// The granted roots, in canonical form, each with its grant order
    /// and time. Read-hot: its own lock.
    grants: Arc<RwLock<BTreeMap<PathBuf, GrantMeta>>>,
    /// The clock a new grant's `added_at` reads: the wall clock in
    /// production, a scripted one in tests that need distinct stamps
    /// without waiting a second between grants.
    now: fn() -> String,
    /// The backing workspace file; `None` while the workspace is
    /// ephemeral.
    backing: Arc<RwLock<Option<Backing>>>,
    /// Serializes ui-state puts so values reach the backing file's actor
    /// in the order they reached memory; see [`Workspace::put_ui_state`].
    ui_state_puts: Arc<tokio::sync::Mutex<()>>,
    /// Serializes the switch operations (open, save-as, duplicate, and
    /// the shutdown close) and grants. Each switch is two phases, open or
    /// create a handle and then swap it in, with awaits between them, and
    /// the same-file guard in [`Workspace::open_file`] reads state a
    /// concurrent switch would change. One guard held across the whole
    /// switch keeps two openers of one file from ever existing (turso
    /// shares one WAL handle per file process-wide, so the second swap's
    /// close would unlink the sidecar the survivor writes to) and keeps a
    /// reload of the current file from landing after the backing moved on.
    switches: Arc<tokio::sync::Mutex<()>>,
    /// Set once by [`Workspace::close_backing`] and never cleared: after
    /// the shutdown close no switch may install a backing nobody would
    /// close, so a switch that loses the race to quit is refused.
    closed: Arc<AtomicBool>,
    /// Where the last-used file is remembered between runs; `None` when
    /// built without a state directory (see [`Workspace::with_state_dir`]).
    pointer: Option<pointer::LastWorkspacePointer>,
    #[cfg(feature = "test-fixtures")]
    pub(crate) stall: Arc<crate::workspace_stall::WriteStall>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            grants: Arc::default(),
            now: now_rfc3339,
            backing: Arc::default(),
            ui_state_puts: Arc::default(),
            switches: Arc::default(),
            closed: Arc::default(),
            pointer: None,
            #[cfg(feature = "test-fixtures")]
            stall: Arc::new(crate::workspace_stall::WriteStall::new()),
        }
    }
}

impl Workspace {
    /// Creates an ephemeral workspace with no grants and no file.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The same workspace reading grant times from `now` instead of the
    /// wall clock, so a test can give consecutive grants distinct stamps.
    #[cfg(test)]
    #[must_use]
    fn with_clock_for_test(mut self, now: fn() -> String) -> Self {
        self.now = now;
        self
    }

    /// Registers `path` as a granted root in memory only: a directory
    /// grants itself, a file grants its parent directory. A new grant
    /// takes the position one past the current maximum and the current
    /// time; a path already granted keeps the order and time it has.
    /// Handlers use [`Workspace::grant_and_persist`], which also mirrors
    /// the grant into the backing file.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::ForbiddenComponent`] when the path contains
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
                added_at: (self.now)(),
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
    /// Returns [`WorkspaceError::ForbiddenComponent`] when the path contains
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
        let text = String::from_utf8(bytes).map_err(|source| WorkspaceError::NotUtf8 { source })?;
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
        #[cfg(feature = "test-fixtures")]
        let _done = self.stall_wait();
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

    /// Lists one level of an existing confined directory. A link to a folder
    /// lists as that folder, any other link as itself; opening either confines.
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
            let own = entry
                .metadata()
                .map_err(|source| WorkspaceError::InspectPath { source })?;
            let metadata = if own.is_symlink() {
                fs::metadata(entry.path())
                    .ok()
                    .filter(fs::Metadata::is_dir)
                    .unwrap_or(own)
            } else {
                own
            };
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
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_close;
#[cfg(test)]
mod tests_reopen;
#[cfg(test)]
mod tests_switch;
