//! The generic in-memory backend.
//!
//! [`MemoryBackend`] implements the former MemStore semantics on the VFS
//! trait surface: bytes keyed by canonical path, writes that materialize
//! their ancestor directories (no `mkdir` needed before a write), and
//! strict removals (absent is `NotFound`; a non-empty directory without
//! `recursive` is an error). `ExecId` attribution is accepted as a no-op:
//! every session shares the one map. It owns only memory and drops with
//! the run.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::{PathReason, VfsError};
use crate::glob::{compile_glob, matches_tokens, validate_glob_pattern};
use crate::path::VfsPath;
use crate::stat::{Entry, FileType, Stat};
use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

/// The storage one backend shares with every session it vends. `BTreeMap`
/// and `BTreeSet` keep listing and glob results ordered without a sort
/// step.
#[derive(Debug)]
struct Tree {
    /// File contents by canonical path.
    files: BTreeMap<String, Vec<u8>>,
    /// Every directory, including each file's ancestors. The root is
    /// always present.
    dirs: BTreeSet<String>,
}

impl Default for Tree {
    fn default() -> Self {
        Tree {
            files: BTreeMap::new(),
            dirs: BTreeSet::from(["/".to_owned()]),
        }
    }
}

/// The ancestor directories of `path`, rootward: `/a/b/c` yields `/a`
/// then `/a/b`. The root itself is always present, so it is never yielded.
fn ancestors(path: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut rest = path;
    while let Some(pos) = rest.rfind('/') {
        if pos == 0 {
            break;
        }
        result.push(&path[..pos]);
        rest = &path[..pos];
    }
    result.reverse();
    result
}

impl Tree {
    /// Whether `path` is a file or a directory.
    fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path) || self.dirs.contains(path)
    }

    /// Whether `path` has any descendants.
    fn has_children(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        self.files.keys().any(|key| key.starts_with(&prefix))
            || self.dirs.iter().any(|key| key.starts_with(&prefix))
    }

    /// The immediate child names of the directory at `path`, sorted.
    fn children(&self, path: &str) -> Vec<String> {
        let prefix = if path == "/" {
            "/".to_owned()
        } else {
            format!("{path}/")
        };
        let mut names = BTreeSet::new();
        for key in self.files.keys().chain(self.dirs.iter()) {
            if let Some(rest) = key.strip_prefix(prefix.as_str())
                && !rest.is_empty()
                && !rest.contains('/')
            {
                names.insert(rest.to_owned());
            }
        }
        names.into_iter().collect()
    }

    /// Metadata for a path known to exist, or `None`. Times and mode are
    /// `None`: the memory backend does not track them, and an invented
    /// mtime would be nondeterministic.
    fn stat_of(&self, path: &str) -> Option<Stat> {
        if let Some(bytes) = self.files.get(path) {
            return Some(Stat {
                file_type: FileType::File,
                size: bytes.len() as u64,
                mode: None,
                modified: None,
                created: None,
            });
        }
        if self.dirs.contains(path) {
            return Some(Stat {
                file_type: FileType::Directory,
                size: 0,
                mode: None,
                modified: None,
                created: None,
            });
        }
        None
    }

    /// Validates that `path` can receive a file: no directory already sits
    /// at `path` and no ancestor is a file. Failure-atomic: callers run
    /// this before any mutation.
    fn check_file_destination(&self, path: &str) -> Result<(), VfsError> {
        if self.dirs.contains(path) {
            return Err(VfsError::IsADirectory {
                path: path.to_owned(),
            });
        }
        if let Some(ancestor) = ancestors(path)
            .into_iter()
            .find(|ancestor| self.files.contains_key(*ancestor))
        {
            return Err(VfsError::NotADirectory {
                path: ancestor.to_owned(),
            });
        }
        Ok(())
    }

    /// Inserts every missing ancestor directory of `path`.
    fn create_ancestors(&mut self, path: &str) {
        for ancestor in ancestors(path) {
            self.dirs.insert(ancestor.to_owned());
        }
    }
}

/// An in-memory [`Vfs`] backend.
///
/// Files are stored in a [`BTreeMap`] keyed by canonical path. Directory
/// listings and glob results come back sorted. Clones share the same
/// storage. `MemoryBackend::default()` returns an empty backend, the same
/// as `new()`.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct MemoryBackend {
    tree: Arc<Mutex<Tree>>,
}

impl MemoryBackend {
    /// Creates an empty in-memory backend.
    #[must_use]
    pub fn new() -> MemoryBackend {
        MemoryBackend::default()
    }
}

impl Vfs for MemoryBackend {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        // Attribution is accepted as a no-op: every session shares the
        // one map, and the claims model above the backend enforces
        // conflicts.
        let _ = cx;
        Ok(Box::new(MemoryAccess {
            tree: Arc::clone(&self.tree),
        }))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        let _ = id;
        Ok(())
    }
}

/// One identity's session with a [`MemoryBackend`]. The identity is
/// dropped on the floor: the map is shared and attribution is a no-op.
struct MemoryAccess {
    tree: Arc<Mutex<Tree>>,
}

impl MemoryAccess {
    /// Poison-safe lock on the storage, held per call.
    fn tree(&self) -> MutexGuard<'_, Tree> {
        self.tree.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl VfsAccess for MemoryAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let tree = self.tree();
        if let Some(bytes) = tree.files.get(path.as_str()) {
            return Ok(bytes.clone());
        }
        if tree.dirs.contains(path.as_str()) {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        Err(VfsError::NotFound {
            path: path.to_string(),
        })
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        let mut tree = self.tree();
        tree.check_file_destination(path.as_str())?;
        tree.create_ancestors(path.as_str());
        tree.files.insert(path.to_string(), contents.to_vec());
        Ok(())
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        let mut tree = self.tree();
        if let Some(bytes) = tree.files.get_mut(path.as_str()) {
            bytes.extend_from_slice(contents);
            return Ok(());
        }
        tree.check_file_destination(path.as_str())?;
        tree.create_ancestors(path.as_str());
        tree.files.insert(path.to_string(), contents.to_vec());
        Ok(())
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let mut tree = self.tree();
        let key = path.as_str();
        if tree.files.remove(key).is_some() {
            return Ok(());
        }
        if !tree.dirs.contains(key) {
            return Err(VfsError::NotFound {
                path: path.to_string(),
            });
        }
        if key == "/" {
            return Err(VfsError::PermissionDenied {
                path: path.to_string(),
                reason: "the namespace root cannot be removed".into(),
            });
        }
        if tree.has_children(key) && !recursive {
            return Err(VfsError::DirectoryNotEmpty {
                path: path.to_string(),
            });
        }
        let prefix = format!("{key}/");
        tree.files.retain(|file, _| !file.starts_with(&prefix));
        tree.dirs.retain(|dir| !dir.starts_with(&prefix));
        tree.dirs.remove(key);
        Ok(())
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        Ok(self.tree().contains(path.as_str()))
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        if let Err(reason) = validate_glob_pattern(pattern) {
            return Err(VfsError::InvalidPath {
                path: pattern.to_owned(),
                reason,
            });
        }
        // Compile once, then reuse the tokens across every key, so the
        // per-key tokenization cost is not repeated while the storage
        // lock is held. Matching itself is bounded and non-backtracking.
        let tokens = compile_glob(pattern.as_bytes());
        let tree = self.tree();
        let mut matches: Vec<String> = tree
            .files
            .keys()
            .chain(tree.dirs.iter())
            .filter(|key| matches_tokens(&tokens, key.as_bytes()))
            .cloned()
            .collect();
        matches.sort_unstable();
        Ok(matches)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        let tree = self.tree();
        let key = path.as_str();
        if tree.files.contains_key(key) {
            return Err(VfsError::NotADirectory {
                path: path.to_string(),
            });
        }
        if !tree.dirs.contains(key) {
            return Err(VfsError::NotFound {
                path: path.to_string(),
            });
        }
        let mut entries = Vec::new();
        for name in tree.children(key) {
            let full = if key == "/" {
                format!("/{name}")
            } else {
                format!("{key}/{name}")
            };
            let Some(stat) = tree.stat_of(&full) else {
                continue; // unreachable: children() only yields existing paths
            };
            entries.push(Entry {
                name,
                stat,
                description: None,
            });
        }
        Ok(entries)
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.tree()
            .stat_of(path.as_str())
            .ok_or_else(|| VfsError::NotFound {
                path: path.to_string(),
            })
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let mut tree = self.tree();
        let key = path.as_str();
        if tree.contains(key) {
            return Err(VfsError::AlreadyExists {
                path: path.to_string(),
            });
        }
        if let Some(file) = ancestors(key)
            .into_iter()
            .find(|ancestor| tree.files.contains_key(*ancestor))
        {
            return Err(VfsError::NotADirectory {
                path: file.to_owned(),
            });
        }
        let missing_ancestors: Vec<&str> = ancestors(key)
            .into_iter()
            .filter(|ancestor| !tree.dirs.contains(*ancestor))
            .collect();
        if !recursive && !missing_ancestors.is_empty() {
            // The field names the path that did not resolve, as the
            // real-filesystem backend reports it: the target the call
            // addressed, not a sentence about its parent.
            return Err(VfsError::NotFound {
                path: path.to_string(),
            });
        }
        tree.create_ancestors(key);
        tree.dirs.insert(key.to_owned());
        Ok(())
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let mut tree = self.tree();
        let source = from.as_str();
        let dest = to.as_str();
        if source == "/" {
            return Err(VfsError::PermissionDenied {
                path: source.to_owned(),
                reason: "the namespace root cannot be renamed".into(),
            });
        }
        if dest.starts_with(&format!("{source}/")) {
            return Err(VfsError::InvalidPath {
                path: source.to_owned(),
                reason: PathReason::IntoDescendant,
            });
        }
        if let Some(bytes) = tree.files.get(source).cloned() {
            tree.check_file_destination(dest)?;
            tree.create_ancestors(dest);
            tree.files.remove(source);
            tree.files.insert(dest.to_owned(), bytes);
            return Ok(());
        }
        if !tree.dirs.contains(source) {
            return Err(VfsError::NotFound {
                path: from.to_string(),
            });
        }
        // A directory moves with its whole subtree. Validation finishes
        // before any mutation, so a failed rename changes nothing.
        if dest == "/" {
            return Err(VfsError::PermissionDenied {
                path: source.to_owned(),
                reason: "a directory cannot be renamed onto the namespace root".into(),
            });
        }
        if tree.files.contains_key(dest) {
            return Err(VfsError::NotADirectory {
                path: to.to_string(),
            });
        }
        if tree.dirs.contains(dest) && tree.has_children(dest) {
            return Err(VfsError::DirectoryNotEmpty {
                path: to.to_string(),
            });
        }
        if let Some(ancestor) = ancestors(dest)
            .into_iter()
            .find(|ancestor| tree.files.contains_key(*ancestor))
        {
            return Err(VfsError::NotADirectory {
                path: ancestor.to_owned(),
            });
        }
        let prefix = format!("{source}/");
        let moved_files: Vec<String> = tree
            .files
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        let moved_dirs: Vec<String> = tree
            .dirs
            .iter()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        for key in moved_files {
            if let Some(bytes) = tree.files.remove(&key) {
                tree.files
                    .insert(format!("{dest}{}", &key[source.len()..]), bytes);
            }
        }
        for key in moved_dirs {
            tree.dirs.remove(&key);
            tree.dirs.insert(format!("{dest}{}", &key[source.len()..]));
        }
        tree.dirs.remove(source);
        tree.dirs.remove(dest);
        tree.create_ancestors(dest);
        tree.dirs.insert(dest.to_owned());
        Ok(())
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let mut tree = self.tree();
        let Some(bytes) = tree.files.get(from.as_str()).cloned() else {
            if tree.dirs.contains(from.as_str()) {
                return Err(VfsError::IsADirectory {
                    path: from.to_string(),
                });
            }
            return Err(VfsError::NotFound {
                path: from.to_string(),
            });
        };
        tree.check_file_destination(to.as_str())?;
        tree.create_ancestors(to.as_str());
        tree.files.insert(to.to_string(), bytes);
        Ok(())
    }
}

#[cfg(test)]
#[path = "memory-tests.rs"]
mod tests;
