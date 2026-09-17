//! The optional workspace file behind a [`Workspace`]: the backing that
//! mirrors the in-memory grant set between sessions, and the open,
//! save-as, duplicate, and window-state operations that swap or write
//! it. The grant set stays the confinement source of truth and nothing
//! here is consulted on a request path; a persist that fails is logged
//! degradation that leaves memory exactly as it is (zone two).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use serde::Serialize;

use crate::error::WorkspaceError;
use crate::workspace_file::{
    GrantRow, WindowState, WorkspaceContents, WorkspaceFile, WorkspaceFileError, empty_ui_state,
    stem_of,
};

use super::{GrantMeta, Workspace, canonicalize_simplified};

#[path = "workspace-ui-state.rs"]
mod ui_state;

/// The display name of a workspace that has no file yet.
pub(crate) const EPHEMERAL_NAME: &str = "Untitled";

/// The open file behind a file-backed workspace.
#[derive(Debug)]
pub(super) struct Backing {
    /// The workspace-file actor (single writer, channel-fed).
    file: WorkspaceFile,
    /// Where the file lives on disk.
    path: PathBuf,
    /// The opaque ui-state values, read from the file at open and
    /// updated on every accepted put; what [`Workspace::ui_state`]
    /// reports.
    ui_state: BTreeMap<&'static str, Option<serde_json::Value>>,
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
    /// grant still returns success. The row carries the order and time
    /// memory assigned; the file assigns its own stored position on
    /// insert, and the in-memory one is what a later save-as writes.
    ///
    /// # Errors
    /// The same as [`Workspace::grant`]; persistence never fails the call.
    pub async fn grant_and_persist(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        let (root, meta) = self.grant_with_meta(path)?;
        if let Some(file) = self.backing_file() {
            let row = GrantRow {
                path: root.clone(),
                position: meta.position,
                added_at: meta.added_at,
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
    /// Opening the file that is already the backing reloads its grants
    /// and ui-state through the existing handle and leaves the backing
    /// as it is: no second handle is opened and nothing is closed.
    ///
    /// Switches run one at a time (see the `switches` field): a second
    /// open of a path the first just made current takes the reload
    /// branch, and an open that arrives after the shutdown close is
    /// refused.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::NotFound`] when the path does not exist,
    /// [`WorkspaceError::WorkspaceFileRefused`] when it is not a workspace
    /// at a supported version, and [`WorkspaceError::WorkspaceFileFailed`]
    /// when it cannot be read or the workspace has been closed for
    /// shutdown.
    pub async fn open_file(&self, path: &Path) -> Result<(), WorkspaceError> {
        let _switch = self.switches.lock().await;
        self.refuse_if_closed("open")?;
        // A second opener of the current file must never exist. turso
        // 0.7.2 keys its process-wide `DATABASE_MANAGER` on OS file
        // identity and hands every connection to the same file one shared
        // WAL handle, so a fresh `WorkspaceFile::open` here would share the
        // live WAL, and `swap_backing` closing the first handle would
        // checkpoint, drop, and unlink the sidecar the survivor keeps
        // appending to: every write after that would be lost at quit.
        // Compare canonical forms so a respelling of the same path (a `.`
        // segment, a case difference on Windows) takes the same branch.
        if let Some((file, current)) = self.backing_parts()
            && let Ok(requested) = canonicalize_simplified(path)
            && canonicalize_simplified(&current).is_ok_and(|current| current == requested)
        {
            return self.reload_current(&file, path).await;
        }
        let file = WorkspaceFile::open(path).await?;
        let contents = match file.contents().await {
            Ok(contents) => contents,
            Err(error) => {
                file.close().await;
                return Err(error.into());
            }
        };
        self.replace_all(contents.grants);
        self.swap_backing(file, path, contents.ui_state).await;
        Ok(())
    }

    /// Creates a new workspace file at `path` holding the current grants
    /// and the previous backing's window state, and makes it the backing.
    /// The previous file, if any, keeps its contents and is closed; its
    /// siblings stay where they are. Save-as moves preferences to a new
    /// name, not the world. The ui-state keys are not carried, in the
    /// file or in memory: the SPA is their one writer and writes them
    /// after a save-as itself.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::WorkspaceFileTaken`] when something
    /// already exists at `path`, and [`WorkspaceError::WorkspaceFileFailed`]
    /// when the file cannot be created or the workspace has been closed
    /// for shutdown.
    pub async fn save_as(&self, path: &Path) -> Result<(), WorkspaceError> {
        let _switch = self.switches.lock().await;
        self.refuse_if_closed("save as")?;
        self.save_as_switched(path).await
    }

    /// [`Workspace::save_as`] with the switch guard already held, so
    /// [`Workspace::duplicate`] can fall back to it without reacquiring.
    async fn save_as_switched(&self, path: &Path) -> Result<(), WorkspaceError> {
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
        self.swap_backing(file, path, contents.ui_state).await;
        Ok(())
    }

    /// Copies the backing file and its siblings to `path`, then makes the
    /// copy the backing; the original is closed and left as it was. The
    /// in-memory ui-state values carry over unchanged, as the grants do:
    /// the copy holds the same rows, and memory stays the source of
    /// truth. An ephemeral workspace has no file to copy, so its
    /// duplicate is a [`Workspace::save_as`]: the current grants land in
    /// a new file.
    ///
    /// # Errors
    /// Returns [`WorkspaceError::WorkspaceFileTaken`] when something
    /// already exists at `path` or a sibling of the copy is already in
    /// the destination folder, and [`WorkspaceError::WorkspaceFileFailed`]
    /// when the copy cannot be made or opened or the workspace has been
    /// closed for shutdown.
    pub async fn duplicate(&self, path: &Path) -> Result<(), WorkspaceError> {
        let _switch = self.switches.lock().await;
        self.refuse_if_closed("duplicate")?;
        let Some(previous) = self.backing_file() else {
            return self.save_as_switched(path).await;
        };
        let file = previous.duplicate_to(path).await?;
        let ui_state = self.ui_state();
        self.swap_backing(file, path, ui_state).await;
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
    /// opened. Rows are stored as they come, position and time included:
    /// the file holds canonical paths, and a root that has vanished from
    /// disk still loads (it lists as `exists: false`) so the user can see
    /// it and revoke it.
    pub(crate) fn replace_all(&self, grants: Vec<GrantRow>) {
        let mut map = self.grants.write().unwrap_or_else(PoisonError::into_inner);
        map.clear();
        for row in grants {
            tracing::info!(root = %row.path.display(), "workspace grant restored from file");
            map.insert(
                row.path,
                GrantMeta {
                    position: row.position,
                    added_at: row.added_at,
                },
            );
        }
    }

    /// Closes the backing file and leaves the workspace ephemeral: the
    /// backing is taken out under the write lock, then its actor is
    /// stopped and awaited, so when the call returns the file on disk is
    /// complete and the `-wal` sidecar is gone. The in-memory grants
    /// stand; only their mirror is let go. Graceful shutdown runs this
    /// through the subsystem's registered task (see
    /// [`crate::handles::register_tasks`]) so a quit leaves exactly one
    /// file to copy or back up. An ephemeral workspace has nothing to
    /// close and returns at once.
    ///
    /// The close is a switch: it waits for any open, save-as, or
    /// duplicate in flight, then marks the workspace closed so no later
    /// switch installs a backing nobody would close. A second call is a
    /// no-op.
    pub async fn close_backing(&self) {
        let _switch = self.switches.lock().await;
        self.closed.store(true, Ordering::Release);
        let previous = self
            .backing
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(previous) = previous {
            previous.file.close().await;
        }
    }

    /// Refuses a switch once [`Workspace::close_backing`] has run. Called
    /// with the switch guard held, so the flag cannot flip underneath.
    /// Expected during teardown, so it logs at debug.
    fn refuse_if_closed(&self, what: &str) -> Result<(), WorkspaceError> {
        if self.closed.load(Ordering::Acquire) {
            tracing::debug!(what, "workspace switch refused: closed for shutdown");
            return Err(WorkspaceFileError::Closed.into());
        }
        Ok(())
    }

    /// Stops the backing file's actor while leaving the backing in place,
    /// so every later persist fails with a closed file and the file
    /// itself can be reopened elsewhere. Exposes the zone-two path to
    /// tests; unlike [`Workspace::close_backing`], the workspace still
    /// reports the file as its backing afterwards.
    #[cfg(any(test, feature = "test-fixtures"))]
    pub async fn close_backing_for_test(&self) {
        if let Some(file) = self.backing_file() {
            file.close().await;
        }
    }

    /// The backing file's handle, if any, so a test can tell whether an
    /// operation kept or replaced it.
    #[cfg(test)]
    pub(super) fn backing_file_for_test(&self) -> Option<WorkspaceFile> {
        self.backing_file()
    }

    /// Holds the switch guard so a test can queue two switches behind it
    /// and release them in a known order; the guard is acquired in
    /// arrival order.
    #[cfg(test)]
    pub(super) async fn hold_switches_for_test(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.switches.lock().await
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

    /// Installs `file` at `path` as the backing holding `ui_state` in
    /// memory, records it as the last-used workspace, and closes the
    /// previous backing, if any, waiting for its connection to go so the
    /// old file is left complete with no sidecar. Called only with the
    /// switch guard held.
    async fn swap_backing(
        &self,
        file: WorkspaceFile,
        path: &Path,
        ui_state: BTreeMap<&'static str, Option<serde_json::Value>>,
    ) {
        let previous = self
            .backing
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(Backing {
                file,
                path: path.to_path_buf(),
                ui_state,
            });
        self.remember(path);
        if let Some(previous) = previous {
            previous.file.close().await;
        }
    }

    /// Re-reads `file`, the current backing, and applies what it holds:
    /// the grants replace every current grant wholesale and the ui-state
    /// map is replaced in place, under the same contract as an open of
    /// any other file. The handle stays where it is. A file that cannot
    /// be read changes nothing. Called only with the switch guard held,
    /// so the backing it re-reads is still the backing when it applies.
    async fn reload_current(
        &self,
        file: &WorkspaceFile,
        path: &Path,
    ) -> Result<(), WorkspaceError> {
        let contents = file.contents().await?;
        self.replace_all(contents.grants);
        if let Some(backing) = self
            .backing
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            backing.ui_state = contents.ui_state;
        }
        self.remember(path);
        Ok(())
    }

    /// The in-memory grants as file rows in grant order, each with its
    /// own position and time, so a save-as writes the workspace's true
    /// history rather than a fresh stamp over a path-sorted list.
    fn grant_rows(&self) -> Vec<GrantRow> {
        let mut rows: Vec<GrantRow> = self
            .grants
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(path, meta)| GrantRow {
                path: path.clone(),
                position: meta.position,
                added_at: meta.added_at.clone(),
            })
            .collect();
        rows.sort_by_key(|row| row.position);
        rows
    }
}
