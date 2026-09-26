//! Directory tree listings: one level of a confined directory, or the
//! granted roots rendered as a synthetic listing. Split from
//! `workspace.rs` to keep that file under the line ceiling.

use std::fs;
use std::path::Path;

use crate::error::WorkspaceError;

use super::confine::modified_ms;
use super::{EntryKind, TreeEntry, TreeListing, Workspace};

impl Workspace {
    /// Lists one level of `path`, or the granted roots when `path` is
    /// `None` or empty. Directories sort before files, each group ordered
    /// by name.
    ///
    /// # Errors
    /// Returns [`WorkspaceError`] when the path is forbidden, outside every
    /// grant, missing, not a directory, or cannot be listed.
    pub(crate) fn tree(&self, path: Option<&Path>) -> Result<TreeListing, WorkspaceError> {
        match path {
            None => Ok(self.grants_listing()),
            Some(path) if path.as_os_str().is_empty() => Ok(self.grants_listing()),
            Some(path) => self.directory_listing(path),
        }
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
