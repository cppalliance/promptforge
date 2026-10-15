//! A fresh real-filesystem folder per test, named from the test, the
//! process, and a counter, so no clock or random generator is read.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Tells apart roots one process makes; nextest runs each test in its own
/// process, so the process id tells those apart.
static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// A folder at `<temp>/promptforge-tests/<crate>/<label>-<pid>-<n>`,
/// removed on drop.
#[derive(Debug)]
pub(crate) struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    /// Makes the folder for the test named `label`, first removing anything
    /// an earlier process with the same id left at that path.
    #[track_caller]
    pub(crate) fn new(label: &str) -> Self {
        let n = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join("promptforge-tests")
            .join(env!("CARGO_PKG_NAME"))
            .join(format!("{label}-{}-{n}", std::process::id()));
        let cleared = match fs::symlink_metadata(&path) {
            Ok(found) if found.is_dir() => fs::remove_dir_all(&path),
            Ok(_) => fs::remove_file(&path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        };
        if let Err(err) = cleared.and_then(|()| fs::create_dir_all(&path)) {
            panic!("the temp root {} must be created: {err}", path.display());
        }
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_dir_all(&self.path)
            && !std::thread::panicking()
        {
            panic!(
                "the temp root {} must be removed: {err}",
                self.path.display()
            );
        }
    }
}

#[test]
fn temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder() {
    let label = "temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder";
    let (first, second) = (TempRoot::new(label), TempRoot::new(label));
    assert_ne!(first.path(), second.path());
    let dropped = first.path().to_owned();
    fs::write(dropped.join("f.txt"), b"x").expect("the first root takes a file");
    drop(first);
    assert!(!dropped.exists(), "{} survived its drop", dropped.display());
    assert!(second.path().is_dir(), "the second root outlives the first");
}
