//! A filesystem-backed [`Store`] that persists virtual files as real files
//! under a caller-provided root directory.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::StoreError;
use super::glob::{compile_glob, matches_tokens};
use super::mem::Store;

/// A [`Store`] backend that persists virtual files as real files on disk.
///
/// Each logical path maps to a relative filesystem path under the configured
/// root directory. Operations are synchronous; the runtime serializes access
/// behind a [`StoreRef`](super::StoreRef) mutex.
///
/// # Examples
/// ```no_run
/// use promptforge_core::store::FileStore;
///
/// let store = FileStore::new("/tmp/my-run")?;
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    /// Creates a new file-backed store rooted at `root`.
    ///
    /// Creates the root directory (and parents) if it does not exist.
    ///
    /// # Errors
    /// Returns [`std::io::Error`] if the directory cannot be created.
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(FileStore { root })
    }

    /// Maps a logical store path to its filesystem location under `root`.
    ///
    /// Returns `Err` if the path would escape `root` (defense in depth;
    /// `StorePath::parse` already rejects traversal at the `StoreRef` layer).
    fn resolve(&self, path: &str) -> Result<PathBuf, StoreError> {
        if path.is_empty() {
            return Err(StoreError::NotFound {
                path: path.to_owned(),
            });
        }
        let mut resolved = self.root.clone();
        for component in path.split('/') {
            if component.is_empty() || component == "." || component == ".." {
                return Err(StoreError::backend(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path component rejected by confinement check",
                )));
            }
            resolved.push(component);
        }
        Ok(resolved)
    }

    /// Ensures the parent directory of `fs_path` exists.
    fn ensure_parent(fs_path: &Path) -> Result<(), StoreError> {
        if let Some(parent) = fs_path.parent() {
            fs::create_dir_all(parent).map_err(StoreError::backend)?;
        }
        Ok(())
    }

    /// Recursively collects all file paths under `dir`, expressed as logical
    /// store paths relative to `root`.
    fn walk_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), StoreError> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(StoreError::backend(e)),
        };
        for entry in entries {
            let entry = entry.map_err(StoreError::backend)?;
            let ft = entry.file_type().map_err(StoreError::backend)?;
            let entry_path = entry.path();
            if ft.is_dir() {
                Self::walk_files(root, &entry_path, out)?;
            } else if ft.is_file()
                && let Some(logical) = Self::to_logical_path(root, &entry_path)
            {
                out.push(logical);
            }
        }
        Ok(())
    }

    /// Converts a filesystem path back to a logical `/`-separated store path
    /// relative to `root`. Returns `None` if the path cannot be represented
    /// (non-UTF-8 components).
    fn to_logical_path(root: &Path, fs_path: &Path) -> Option<String> {
        let relative = fs_path.strip_prefix(root).ok()?;
        let mut segments = Vec::new();
        for component in relative.components() {
            match component {
                std::path::Component::Normal(s) => {
                    segments.push(s.to_str()?);
                }
                _ => return None,
            }
        }
        if segments.is_empty() {
            return None;
        }
        Some(segments.join("/"))
    }
}

impl Store for FileStore {
    fn write(&mut self, path: &str, contents: &str) -> Result<(), StoreError> {
        let fs_path = self.resolve(path)?;
        Self::ensure_parent(&fs_path)?;
        fs::write(&fs_path, contents).map_err(StoreError::backend)
    }

    fn append(&mut self, path: &str, contents: &str) -> Result<(), StoreError> {
        let fs_path = self.resolve(path)?;
        Self::ensure_parent(&fs_path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&fs_path)
            .map_err(StoreError::backend)?;
        file.write_all(contents.as_bytes())
            .map_err(StoreError::backend)
    }

    fn read(&self, path: &str) -> Result<String, StoreError> {
        let fs_path = self.resolve(path)?;
        match fs::read_to_string(&fs_path) {
            Ok(contents) => Ok(contents),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StoreError::NotFound {
                path: path.to_owned(),
            }),
            Err(e) => Err(StoreError::backend(e)),
        }
    }

    fn str_replace(&mut self, path: &str, old: &str, new: &str) -> Result<(), StoreError> {
        let fs_path = self.resolve(path)?;
        let contents = match fs::read_to_string(&fs_path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound {
                    path: path.to_owned(),
                });
            }
            Err(e) => return Err(StoreError::backend(e)),
        };
        let count = contents.matches(old).count();
        match count {
            0 => Err(StoreError::AnchorNotFound {
                path: path.to_owned(),
                anchor: old.to_owned(),
            }),
            1 => {
                let replaced = contents.replacen(old, new, 1);
                fs::write(&fs_path, replaced).map_err(StoreError::backend)
            }
            _ => Err(StoreError::AnchorAmbiguous {
                path: path.to_owned(),
                anchor: old.to_owned(),
                count,
            }),
        }
    }

    fn delete(&mut self, path: &str) -> Result<(), StoreError> {
        let fs_path = self.resolve(path)?;
        match fs::remove_file(&fs_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StoreError::NotFound {
                path: path.to_owned(),
            }),
            Err(e) => Err(StoreError::backend(e)),
        }
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, StoreError> {
        let mut paths = Vec::new();
        Self::walk_files(&self.root, &self.root, &mut paths)?;
        let tokens = compile_glob(pattern.as_bytes());
        paths.retain(|p| matches_tokens(&tokens, p.as_bytes()));
        paths.sort();
        Ok(paths)
    }

    fn exists(&self, path: &str) -> Result<bool, StoreError> {
        let fs_path = self.resolve(path)?;
        Ok(fs_path.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_store() -> (TempDir, FileStore) {
        let dir = TempDir::new().expect("create temp dir");
        let store = FileStore::new(dir.path()).expect("create FileStore");
        (dir, store)
    }

    #[test]
    fn write_and_read() {
        let (_dir, mut store) = temp_store();
        store.write("hello.txt", "world").expect("write");
        assert_eq!(store.read("hello.txt").expect("read"), "world");
    }

    #[test]
    fn write_creates_parent_dirs() {
        let (_dir, mut store) = temp_store();
        store.write("a/b/c.txt", "deep").expect("write nested");
        assert_eq!(store.read("a/b/c.txt").expect("read"), "deep");
    }

    #[test]
    fn append_creates_and_accumulates() {
        let (_dir, mut store) = temp_store();
        store.append("log.txt", "first\n").expect("append 1");
        store.append("log.txt", "second").expect("append 2");
        assert_eq!(store.read("log.txt").expect("read"), "first\nsecond");
    }

    #[test]
    fn read_missing_file_returns_not_found() {
        let (_dir, store) = temp_store();
        let err = store.read("ghost.txt").expect_err("should error");
        assert!(err.is_not_found());
    }

    #[test]
    fn str_replace_single_occurrence() {
        let (_dir, mut store) = temp_store();
        store.write("a.txt", "the quick brown fox").expect("write");
        store
            .str_replace("a.txt", "quick", "slow")
            .expect("replace");
        assert_eq!(store.read("a.txt").expect("read"), "the slow brown fox");
    }

    #[test]
    fn str_replace_missing_anchor() {
        let (_dir, mut store) = temp_store();
        store.write("a.txt", "hello").expect("write");
        let err = store
            .str_replace("a.txt", "missing", "x")
            .expect_err("should error");
        assert!(matches!(err, StoreError::AnchorNotFound { .. }));
    }

    #[test]
    fn str_replace_ambiguous_anchor() {
        let (_dir, mut store) = temp_store();
        store.write("a.txt", "aa").expect("write");
        let err = store
            .str_replace("a.txt", "a", "b")
            .expect_err("should error");
        assert!(matches!(err, StoreError::AnchorAmbiguous { .. }));
    }

    #[test]
    fn delete_removes_file() {
        let (_dir, mut store) = temp_store();
        store.write("temp.txt", "data").expect("write");
        store.delete("temp.txt").expect("delete");
        assert!(!store.exists("temp.txt").expect("exists"));
    }

    #[test]
    fn delete_missing_returns_not_found() {
        let (_dir, mut store) = temp_store();
        let err = store.delete("ghost.txt").expect_err("should error");
        assert!(err.is_not_found());
    }

    #[test]
    fn glob_matches_patterns() {
        let (_dir, mut store) = temp_store();
        store.write("src/a.rs", "").expect("write");
        store.write("src/b.rs", "").expect("write");
        store.write("src/deep/c.rs", "").expect("write");
        store.write("notes.md", "").expect("write");

        let matched = store.glob("src/*.rs").expect("glob");
        assert_eq!(matched, vec!["src/a.rs", "src/b.rs"]);

        let all_rs = store.glob("**/*.rs").expect("glob");
        assert_eq!(all_rs, vec!["src/a.rs", "src/b.rs", "src/deep/c.rs"]);

        let everything = store.glob("**").expect("glob");
        assert_eq!(everything.len(), 4);
    }

    #[test]
    fn exists_reports_correctly() {
        let (_dir, mut store) = temp_store();
        assert!(!store.exists("a.txt").expect("exists before"));
        store.write("a.txt", "hi").expect("write");
        assert!(store.exists("a.txt").expect("exists after"));
    }

    #[test]
    fn confinement_rejects_traversal() {
        let (_dir, store) = temp_store();
        let err = store.read("../escape.txt").expect_err("should reject");
        assert!(matches!(err, StoreError::Backend { .. }));
    }

    #[test]
    fn confinement_rejects_dot_segment() {
        let (_dir, store) = temp_store();
        let err = store.read("a/./b.txt").expect_err("should reject");
        assert!(matches!(err, StoreError::Backend { .. }));
    }

    #[test]
    fn constructor_creates_missing_root() {
        let dir = TempDir::new().expect("create temp dir");
        let nested = dir.path().join("deep").join("nested");
        let _store = FileStore::new(&nested).expect("should create dirs");
        assert!(nested.is_dir());
    }
}
