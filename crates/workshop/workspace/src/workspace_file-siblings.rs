//! The siblings a workspace grows beside its file and how a duplicate
//! carries them: which names travel, the refusal that keeps one
//! workspace's siblings from being merged into another's, and the copy
//! and cleanup of a planned set.

use std::path::{Path, PathBuf};
use std::{fs, io};

use super::WorkspaceFileError;

/// The entries a workspace may grow beside its file, each created
/// lazily by its own project; duplicate copies whichever exist, minus
/// [`DERIVED_SIBLINGS`].
const WORKSPACE_SIBLINGS: &[&str] = &["agents", "runs", "terminals", "plugins", "index.db"];
/// Siblings derived from the others and rebuilt on demand; duplicate
/// never copies them.
const DERIVED_SIBLINGS: &[&str] = &["index.db"];

/// Lists the `(from, to)` pairs of the siblings a duplicate of `source`
/// at `destination` must copy, refusing before anything is written when
/// a `to` already exists: that is another workspace's data, never to be
/// merged into. Only the named workspace siblings travel: the derived
/// index never does, and nothing else in the folder is the workspace's
/// to copy. Two files in one folder share their siblings by convention,
/// so a same-folder duplicate copies none.
pub(super) fn plan_siblings(
    source: &Path,
    destination: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>, WorkspaceFileError> {
    let (Some(source_dir), Some(destination_dir)) = (source.parent(), destination.parent()) else {
        return Ok(Vec::new());
    };
    if source_dir == destination_dir {
        return Ok(Vec::new());
    }
    let mut siblings = Vec::new();
    for name in WORKSPACE_SIBLINGS {
        if DERIVED_SIBLINGS.contains(name) {
            continue;
        }
        let from = source_dir.join(name);
        if !from.exists() {
            continue;
        }
        let to = destination_dir.join(name);
        if to.exists() {
            return Err(already_taken(
                "destination folder already holds a workspace sibling of the same name",
            ));
        }
        siblings.push((from, to));
    }
    Ok(siblings)
}

/// Copies every planned sibling of a duplicate; when one copy fails,
/// removes the siblings already copied and the copied workspace file at
/// `destination`, so a failed duplicate leaves nothing behind. A failed
/// removal cannot say more than the copy failure did.
pub(super) fn copy_siblings_or_clean_up(
    siblings: &[(PathBuf, PathBuf)],
    destination: &Path,
) -> io::Result<()> {
    if let Err(source) = copy_siblings(siblings) {
        for (_, to) in siblings {
            let _ = remove_sibling(to);
        }
        let _ = fs::remove_file(destination);
        return Err(source);
    }
    Ok(())
}

/// An `AlreadyExists` I/O refusal carrying `message`: the shape both
/// create and duplicate use to refuse a path that is already taken.
pub(super) fn already_taken(message: &'static str) -> WorkspaceFileError {
    WorkspaceFileError::Io {
        source: io::Error::new(io::ErrorKind::AlreadyExists, message),
    }
}

/// Copies every planned sibling, a directory tree or a single file.
fn copy_siblings(siblings: &[(PathBuf, PathBuf)]) -> io::Result<()> {
    for (from, to) in siblings {
        if from.is_dir() {
            copy_dir_recursive(from, to)?;
        } else {
            fs::copy(from, to)?;
        }
    }
    Ok(())
}

/// Removes a copied sibling, a directory tree or a single file.
fn remove_sibling(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Copies the tree under `from` to `to`, creating directories as needed.
fn copy_dir_recursive(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
