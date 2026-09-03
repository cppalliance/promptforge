//! Crash-safe file writes shared by the workspace write endpoint and the
//! menu's model-memory persistence: each write lands in a uniquely named
//! sibling temp file, is synced to disk, and is renamed over the target,
//! so a crash at any moment leaves either the old contents or the new,
//! never a truncation. The startup sweep removes temp files orphaned by
//! a crash between the write and the rename; it covers directories the
//! server owns at boot (the configured state directory) - workspace
//! grants are runtime-only, so a granted directory cannot be swept
//! before it is granted.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Suffix every atomic-write temp file carries; the sweep recognizes
/// orphans by it.
const TEMP_SUFFIX: &str = ".pf-tmp";

/// Fixed temp name the pre-helper menu scheme wrote beside the workshop
/// state file; a crash under that scheme left this orphan, which the
/// sweep also removes.
const LEGACY_TEMP_NAME: &str = "workshop-state.json.tmp";

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

/// Writes `bytes` to `path` atomically: the bytes land in a unique
/// sibling temp file, are synced to disk with `sync_data`, and the temp
/// is renamed over `path`. On failure the temp file is removed and the
/// target keeps its previous contents.
///
/// # Errors
/// Returns [`io::ErrorKind::InvalidInput`] when `path` has no file name,
/// and otherwise the underlying I/O error when the create, write, sync,
/// or rename fails.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let Some(temp) = temp_path(path) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "write target has no file name",
        ));
    };
    let result = fs::File::create(&temp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_data()
        })
        .and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        // Best-effort: when create itself failed there is nothing to
        // remove, and anything remove misses here the next startup
        // sweep catches.
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Removes temp files a crashed write left behind in `dir`, without
/// recursing. A missing directory is tolerated silently: the server has
/// simply never written there. Every other failure - an unreadable
/// directory or entry, an unremovable file - is logged and tolerated:
/// the sweep is cleanup and must never cost startup.
pub(crate) fn sweep_orphaned_temps(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(
                    %error,
                    dir = %dir.display(),
                    "orphaned temp sweep skipped: directory unreadable"
                );
            }
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(
                    %error,
                    dir = %dir.display(),
                    "orphaned temp sweep: entry unreadable; skipped"
                );
                continue;
            }
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(TEMP_SUFFIX) && name != LEGACY_TEMP_NAME {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {
                tracing::info!(
                    path = %entry.path().display(),
                    "removed a temp file orphaned by a crashed write"
                );
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    path = %entry.path().display(),
                    "orphaned temp file could not be removed; left in place"
                );
            }
        }
    }
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
        let target = dir.path().join("state.json");
        write_atomic(&target, b"one").expect("the first write succeeds");
        assert_eq!(fs::read(&target).expect("readable"), b"one");
        write_atomic(&target, b"two").expect("the overwrite succeeds");
        assert_eq!(fs::read(&target).expect("readable"), b"two");
        assert_eq!(dir_names(dir.path()), ["state.json"]);
    }

    #[test]
    fn a_failed_rename_removes_the_temp_and_reports_the_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        // A directory in the target's place fails the rename after the
        // temp is fully written, exercising the cleanup path.
        let target = dir.path().join("state.json");
        fs::create_dir(&target).expect("directory in the target's place");
        write_atomic(&target, b"payload").expect_err("renaming over a directory must fail");
        assert_eq!(dir_names(dir.path()), ["state.json"]);
        assert!(target.is_dir(), "the failed write touched nothing");
    }

    #[test]
    fn a_missing_parent_directory_fails_without_residue() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let target = dir.path().join("missing").join("state.json");
        write_atomic(&target, b"payload").expect_err("an uncreatable temp must fail");
        assert!(dir_names(dir.path()).is_empty(), "nothing was created");
    }

    #[test]
    fn a_crash_orphan_spares_the_target_and_the_sweep_removes_it() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let target = dir.path().join("state.json");
        write_atomic(&target, b"good").expect("the seed write succeeds");
        // A crash between the temp write and the rename leaves exactly
        // this: a fully or partially written temp beside an intact target.
        let orphan = dir.path().join(format!("state.json.999-0{TEMP_SUFFIX}"));
        fs::write(&orphan, b"partial").expect("the simulated crash residue writes");
        assert_eq!(
            fs::read(&target).expect("readable"),
            b"good",
            "a crash before the rename never touches the target"
        );
        sweep_orphaned_temps(dir.path());
        assert_eq!(
            dir_names(dir.path()),
            ["state.json"],
            "the sweep removes the orphan and spares the real file"
        );
    }

    #[test]
    fn the_sweep_removes_a_legacy_menu_temp_orphan() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let target = dir.path().join("workshop-state.json");
        fs::write(&target, b"good").expect("the seed write succeeds");
        // A crash under the pre-helper menu scheme leaves exactly this:
        // the fixed-name temp beside an intact state file.
        let orphan = dir.path().join(LEGACY_TEMP_NAME);
        fs::write(&orphan, b"partial").expect("the simulated legacy residue writes");
        sweep_orphaned_temps(dir.path());
        assert_eq!(
            dir_names(dir.path()),
            ["workshop-state.json"],
            "the sweep removes the legacy orphan and spares the real file"
        );
    }

    #[test]
    fn the_sweep_tolerates_a_missing_directory() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        sweep_orphaned_temps(&dir.path().join("never-created"));
    }

    #[test]
    fn concurrent_writers_each_get_a_distinct_temp_name() {
        let target = Path::new("dir").join("state.json");
        let first = temp_path(&target).expect("a named target derives a temp");
        let second = temp_path(&target).expect("a named target derives a temp");
        assert_ne!(
            first, second,
            "two writes to one target must never share a temp file"
        );
    }
}
