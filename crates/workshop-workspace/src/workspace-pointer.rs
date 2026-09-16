//! The last-workspace pointer: a plain-text file in the state directory
//! naming the workspace file that was open when the server last ran, so
//! launch reopens it. It is written after every successful switch (open,
//! save-as, duplicate) and read once at boot. Everything here is zone
//! two: a pointer that cannot be written costs the next launch its
//! reopen, and a pointer that cannot be read or followed starts the
//! server ephemeral. Boot waits for the reopen so readiness means the
//! grants are already in place, but it never fails for it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::Workspace;

/// The pointer file's name inside the state directory.
const POINTER_FILE: &str = "last-workspace";

/// Where the last-used workspace file's path is remembered between runs:
/// `state_dir/last-workspace`, holding the path as UTF-8 text.
#[derive(Debug, Clone)]
pub(super) struct LastWorkspacePointer {
    /// The pointer file itself.
    path: PathBuf,
}

impl LastWorkspacePointer {
    /// The pointer inside `state_dir`; nothing is read or created yet.
    pub(super) fn new(state_dir: &Path) -> Self {
        Self {
            path: state_dir.join(POINTER_FILE),
        }
    }

    /// Records `workspace` as the last-used workspace file, atomically,
    /// creating the state directory on a first run.
    ///
    /// # Errors
    /// Returns the I/O failure when the path is not UTF-8 or the file
    /// cannot be written.
    pub(super) fn write(&self, workspace: &Path) -> io::Result<()> {
        let text = workspace.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace path is not UTF-8; a UTF-8 path is required to remember it",
            )
        })?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        workshop_support::write_atomic(&self.path, text.as_bytes())
    }

    /// The remembered workspace file, or `None` when there is nothing
    /// usable to follow. A missing pointer is the ordinary first launch
    /// and logs nothing; unreadable, non-UTF-8, or empty content logs a
    /// warning. A trailing newline from a hand edit is tolerated.
    pub(super) fn read(&self) -> Option<PathBuf> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(
                    %error,
                    pointer = %self.path.display(),
                    "last-workspace pointer unreadable; starting ephemeral"
                );
                return None;
            }
        };
        let Ok(text) = String::from_utf8(bytes) else {
            tracing::warn!(
                pointer = %self.path.display(),
                "last-workspace pointer is not UTF-8 text; starting ephemeral"
            );
            return None;
        };
        let trimmed = text.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            tracing::warn!(
                pointer = %self.path.display(),
                "last-workspace pointer is empty; starting ephemeral"
            );
            return None;
        }
        Some(PathBuf::from(trimmed))
    }
}

impl Workspace {
    /// An ephemeral workspace that remembers its last-used file in
    /// `state_dir`: every successful open, save-as, or duplicate records
    /// the new file there, and [`Workspace::reopen_last`] follows the
    /// record at boot. Nothing is read or written until then.
    #[must_use]
    pub fn with_state_dir(state_dir: &Path) -> Self {
        Self {
            pointer: Some(LastWorkspacePointer::new(state_dir)),
            ..Self::default()
        }
    }

    /// Reopens the workspace file the pointer names, if any, and returns
    /// whether one was reopened. This never fails: a workspace built
    /// without a state directory, a missing or corrupt pointer, a target
    /// that has vanished, and a file that is refused all log and leave
    /// the workspace ephemeral, so boot goes on regardless.
    pub async fn reopen_last(&self) -> bool {
        let Some(path) = self.pointer.as_ref().and_then(LastWorkspacePointer::read) else {
            return false;
        };
        match self.open_file(&path).await {
            Ok(()) => {
                tracing::info!(file = %path.display(), "last-used workspace reopened");
                true
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    file = %path.display(),
                    "last-used workspace not reopened; starting ephemeral"
                );
                false
            }
        }
    }

    /// Records `path` as the last-used workspace file when a state
    /// directory is configured. A write that fails is logged; the switch
    /// that just happened stands.
    pub(super) fn remember(&self, path: &Path) {
        if let Some(pointer) = &self.pointer
            && let Err(error) = pointer.write(path)
        {
            tracing::warn!(
                %error,
                file = %path.display(),
                "last-workspace pointer not written; the next launch starts ephemeral"
            );
        }
    }
}
