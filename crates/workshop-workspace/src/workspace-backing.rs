//! The optional workspace file behind a [`Workspace`]: the backing that
//! mirrors the in-memory grant set between sessions, and the open,
//! save-as, duplicate, and window-state operations that swap or write
//! it. The grant set stays the confinement source of truth and nothing
//! here is consulted on a request path; a persist that fails is logged
//! degradation that leaves memory exactly as it is (zone two).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use serde::Serialize;

use crate::error::WorkspaceError;
use crate::workspace_file::{
    GrantRow, WindowState, WorkspaceContents, WorkspaceFile, empty_ui_state, now_rfc3339, stem_of,
};

use super::Workspace;

/// The display name of a workspace that has no file yet.
pub(crate) const EPHEMERAL_NAME: &str = "Untitled";

/// The open file behind a file-backed workspace.
#[derive(Debug)]
pub(super) struct Backing {
    /// The workspace-file actor (single writer, channel-fed).
    file: WorkspaceFile,
    /// Where the file lives on disk.
    path: PathBuf,
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

/// The workspace as a whole: its file, if any, and what it holds.
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

impl Workspace {
    /// Registers `path` as a granted root and mirrors the grant into the
    /// backing file when one is open. Memory is updated first and stands
    /// whatever the file does: a persist that fails is logged, and the
    /// grant still returns success.
    ///
    /// # Errors
    /// The same as [`Workspace::grant`]; persistence never fails the call.
    pub async fn grant_and_persist(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        let root = self.grant(path)?;
        if let Some(file) = self.backing_file() {
            let row = GrantRow {
                path: root.clone(),
                position: 0,
                added_at: now_rfc3339(),
            };
            if let Err(error) = file.add_grant(row).await {
                tracing::warn!(
                    %error,
                    root = %root.display(),
                    file = %file.path().display(),
                    "grant not persisted to the workspace file; the in-memory grant stands"
                );
            }
        }
        Ok(root)
    }

    /// Removes `path` from the granted roots and mirrors the removal into
    /// the backing file when one is open. Memory is updated first; a
    /// persist that fails is logged, and the revoke still returns
    /// success.
    ///
    /// # Errors
    /// The same as [`Workspace::revoke`]; persistence never fails the
    /// call.
    pub async fn revoke_and_persist(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        let root = self.revoke(path)?;
        if let Some(file) = self.backing_file()
            && let Err(error) = file.remove_grant(&root).await
        {
            tracing::warn!(
                %error,
                root = %root.display(),
                file = %file.path().display(),
                "revoke not persisted to the workspace file; the in-memory revoke stands"
            );
        }
        Ok(root)
    }

    /// Opens the workspace file at `path` and makes it the backing: the
    /// file's grants replace every current grant wholesale, and the
    /// previous backing, if any, is closed. A file that fails validation
    /// changes nothing; the current grants and backing stay as they were.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::NotFound`] when the path does not exist,
    /// [`WorkspaceError::WorkspaceFileRefused`] when it is not a workspace
    /// at a supported version, and [`WorkspaceError::WorkspaceFileFailed`]
    /// when it cannot be read.
    pub async fn open_file(&self, path: &Path) -> Result<(), WorkspaceError> {
        let file = WorkspaceFile::open(path).await?;
        let contents = match file.contents().await {
            Ok(contents) => contents,
            Err(error) => {
                file.close().await;
                return Err(error.into());
            }
        };
        self.replace_all(contents.grants);
        self.swap_backing(file, path).await;
        Ok(())
    }

    /// Creates a new workspace file at `path` holding the current grants
    /// and the previous backing's window state, and makes it the backing.
    /// The previous file, if any, keeps its contents and is closed; its
    /// siblings stay where they are. Save-as moves preferences to a new
    /// name, not the world. The ui-state keys are not carried: the SPA
    /// is their one writer and writes them after a save-as itself.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::WorkspaceFileTaken`] when something
    /// already exists at `path`, and [`WorkspaceError::WorkspaceFileFailed`]
    /// when the file cannot be created.
    pub async fn save_as(&self, path: &Path) -> Result<(), WorkspaceError> {
        let window_state = match self.backing_file() {
            Some(previous) => match previous.contents().await {
                Ok(contents) => contents.window_state,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        file = %previous.path().display(),
                        "previous workspace file unreadable; saving as without its window state"
                    );
                    None
                }
            },
            None => None,
        };
        let contents = WorkspaceContents {
            name: stem_of(path),
            grants: self.grant_rows(),
            window_state,
            ui_state: empty_ui_state(),
        };
        let file = WorkspaceFile::create(path, &contents).await?;
        self.swap_backing(file, path).await;
        Ok(())
    }

    /// Copies the backing file and its siblings to `path`, then makes the
    /// copy the backing; the original is closed and left as it was. An
    /// ephemeral workspace has no file to copy, so its duplicate is a
    /// [`Workspace::save_as`]: the current grants land in a new file.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::WorkspaceFileTaken`] when something
    /// already exists at `path` or a sibling of the copy is already in
    /// the destination folder, and [`WorkspaceError::WorkspaceFileFailed`]
    /// when the copy cannot be made or opened.
    pub async fn duplicate(&self, path: &Path) -> Result<(), WorkspaceError> {
        let Some(previous) = self.backing_file() else {
            return self.save_as(path).await;
        };
        let file = previous.duplicate_to(path).await?;
        self.swap_backing(file, path).await;
        Ok(())
    }

    /// Saves the window geometry into the backing file. Returns
    /// `Ok(false)` without writing when the workspace is ephemeral: an
    /// unsaved workspace has nowhere to keep it.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::WorkspaceFileFailed`] when the write
    /// fails.
    pub async fn put_window_state(&self, state: WindowState) -> Result<bool, WorkspaceError> {
        let Some(file) = self.backing_file() else {
            return Ok(false);
        };
        file.put_window_state(state).await?;
        Ok(true)
    }

    /// The workspace as a whole. The grants come from memory, the
    /// confinement source of truth; the name and window state come from
    /// the file. A file that cannot be read degrades to its stem and no
    /// window state rather than failing the call.
    pub async fn current(&self) -> WorkspaceSummary {
        let grants = self
            .granted_roots()
            .into_iter()
            .map(|path| GrantEntry {
                exists: fs::metadata(&path).is_ok(),
                path,
            })
            .collect();
        let Some((file, path)) = self.backing_parts() else {
            return WorkspaceSummary {
                path: None,
                name: EPHEMERAL_NAME.to_string(),
                grants,
                window_state: None,
            };
        };
        let (name, window_state) = match file.contents().await {
            Ok(contents) => (contents.name, contents.window_state),
            Err(error) => {
                tracing::warn!(
                    %error,
                    file = %path.display(),
                    "workspace file unreadable; reporting its stem and no window state"
                );
                (stem_of(&path), None)
            }
        };
        WorkspaceSummary {
            path: Some(path),
            name,
            grants,
            window_state,
        }
    }

    /// Replaces every grant with `grants`, the contents of a file being
    /// opened. Rows are stored as they come: the file holds canonical
    /// paths, and a root that has vanished from disk still loads (it
    /// lists as `exists: false`) so the user can see it and revoke it.
    pub(crate) fn replace_all(&self, grants: Vec<GrantRow>) {
        let mut set = self.grants.write().unwrap_or_else(PoisonError::into_inner);
        set.clear();
        for row in grants {
            tracing::info!(root = %row.path.display(), "workspace grant restored from file");
            set.insert(row.path);
        }
    }

    /// Stops the backing file's actor while leaving the backing in place,
    /// so every later persist fails with a closed file and the file
    /// itself can be reopened elsewhere. Exposes the zone-two path to
    /// tests.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub async fn close_backing_for_test(&self) {
        if let Some(file) = self.backing_file() {
            file.close().await;
        }
    }

    /// A handle to the backing file, if any. The lock is released before
    /// the caller awaits anything.
    fn backing_file(&self) -> Option<WorkspaceFile> {
        self.backing_parts().map(|(file, _)| file)
    }

    /// The backing file's handle and path, if any.
    fn backing_parts(&self) -> Option<(WorkspaceFile, PathBuf)> {
        self.backing
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|backing| (backing.file.clone(), backing.path.clone()))
    }

    /// Installs `file` at `path` as the backing, records it as the
    /// last-used workspace, and closes the previous backing, if any,
    /// waiting for its connection to go so the old file is left complete
    /// with no sidecar.
    async fn swap_backing(&self, file: WorkspaceFile, path: &Path) {
        let previous = self
            .backing
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(Backing {
                file,
                path: path.to_path_buf(),
            });
        self.remember(path);
        if let Some(previous) = previous {
            previous.file.close().await;
        }
    }

    /// The in-memory grants as file rows in canonical order, all stamped
    /// with the current time: memory keeps no grant times of its own.
    fn grant_rows(&self) -> Vec<GrantRow> {
        let added_at = now_rfc3339();
        self.granted_roots()
            .into_iter()
            .enumerate()
            .map(|(index, path)| GrantRow {
                path,
                position: u32::try_from(index).unwrap_or(u32::MAX),
                added_at: added_at.clone(),
            })
            .collect()
    }
}
