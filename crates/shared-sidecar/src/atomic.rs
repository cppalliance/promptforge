//! Crash-safe, owner-only file writes for the connection file: each write
//! lands in a uniquely named sibling temp file, is synced to disk, and is
//! renamed over the target, so a crash at any moment leaves either the old
//! contents or the new, never a truncation. The pattern mirrors
//! workshop-server's `atomic.rs`; this crate reimplements it rather than
//! depending on a server crate, and adds the owner-only permission the
//! bearer-carrying connection file needs.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Suffix every atomic-write temp file carries.
const TEMP_SUFFIX: &str = ".pf-tmp";

/// Process-wide counter making each temp name unique, so two concurrent
/// writes to the same target never interleave inside one temp file: each
/// writer fills its own temp completely and the last rename wins whole.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The unique sibling temp path for one write to `path`, or `None` when
/// `path` has no file name to derive it from.
fn temp_path(path: &Path) -> Option<PathBuf> {
    let mut name = path.file_name()?.to_os_string();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    name.push(format!(".{}-{counter}{TEMP_SUFFIX}", std::process::id()));
    Some(path.with_file_name(name))
}

/// Writes `bytes` to `path` atomically with owner-only permissions: the
/// bytes land in a unique sibling temp file created with mode `0600` on
/// Unix (the rename preserves it, so the target never exists with looser
/// permissions), are synced to disk, and the temp is renamed over `path`.
/// On failure the temp file is removed and the target keeps its previous
/// contents.
///
/// On Windows there are no mode bits; the file lives under the user
/// profile, whose ACL already restricts it to the owner. Best-effort by
/// design.
pub(crate) fn write_atomic_owner_only(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let Some(temp) = temp_path(path) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "write target has no file name",
        ));
    };
    let result = write_temp_then_rename(&temp, path, bytes);
    if result.is_err() {
        // Best-effort: when the create itself failed there is nothing to
        // remove.
        let _ = fs::remove_file(&temp);
    }
    result
}

/// The Unix write: the temp file is born owner-only.
#[cfg(unix)]
fn write_temp_then_rename(temp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp)?;
    file.write_all(bytes)?;
    file.sync_data()?;
    fs::rename(temp, path)
}

/// The Windows write: no mode bits exist; the user profile's ACL is the
/// permission boundary.
#[cfg(not(unix))]
fn write_temp_then_rename(temp: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(temp)?;
    file.write_all(bytes)?;
    file.sync_data()?;
    fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sorted file names in `dir`.
    fn dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("the test directory is listable")
            .map(|entry| {
                entry
                    .expect("the test entry is readable")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_write_lands_whole_and_leaves_no_temp_behind() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let target = dir.path().join("gateway.json");
        write_atomic_owner_only(&target, b"one").expect("the first write succeeds");
        assert_eq!(fs::read(&target).expect("readable"), b"one");
        write_atomic_owner_only(&target, b"two").expect("the overwrite succeeds");
        assert_eq!(fs::read(&target).expect("readable"), b"two");
        assert_eq!(dir_names(dir.path()), ["gateway.json"]);
    }

    #[test]
    fn a_failed_rename_removes_the_temp_and_reports_the_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        // A directory in the target's place fails the rename after the
        // temp is fully written, exercising the cleanup path.
        let target = dir.path().join("gateway.json");
        fs::create_dir(&target).expect("directory in the target's place");
        write_atomic_owner_only(&target, b"payload")
            .expect_err("renaming over a directory must fail");
        assert_eq!(dir_names(dir.path()), ["gateway.json"]);
        assert!(target.is_dir(), "the failed write touched nothing");
    }

    #[cfg(unix)]
    #[test]
    fn the_written_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let target = dir.path().join("gateway.json");
        write_atomic_owner_only(&target, b"payload").expect("the write succeeds");
        let mode = target.metadata().expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the bearer key file is owner-only");
    }
}
