//! The generic in-memory backend.
//!
//! [`MemoryBackend`] carries the former MemStore semantics onto the VFS
//! trait surface: bytes keyed by canonical path, writes that materialize
//! their ancestor directories (no `mkdir` needed before a write), and
//! strict removals (absent is `NotFound`; a non-empty directory without
//! `recursive` is an error). `ExecId` attribution is accepted as a no-op:
//! every session shares the one map. It holds no resources and drops with
//! the run.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::VfsError;
use crate::glob::{MAX_GLOB_PATTERN_BYTES, compile_glob, matches_tokens, validate_glob_grammar};
use crate::path::VfsPath;
use crate::traits::{ExecId, Vfs, VfsAccess};
use crate::types::{Entry, FileType, Stat};

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
            return Err(VfsError::IsADirectory(path.to_owned()));
        }
        if let Some(ancestor) = ancestors(path)
            .into_iter()
            .find(|ancestor| self.files.contains_key(*ancestor))
        {
            return Err(VfsError::NotADirectory(ancestor.to_owned()));
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
/// Files live in a [`BTreeMap`] keyed by canonical path, so listing and
/// glob results are ordered without a sort step. Clones share the same
/// storage. The zero value (`Default`) is a meaningful empty backend.
///
/// # Examples
/// ```
/// use shared_vfs::{MemoryBackend, VfsRef};
///
/// let vfs = VfsRef::new(MemoryBackend::new());
/// let access = vfs.acquire();
/// access.write("/notes.md", b"todo")?;
/// assert_eq!(access.read("/notes.md")?, b"todo");
/// # Ok::<(), shared_vfs::VfsError>(())
/// ```
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
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError> {
        // Attribution is accepted as a no-op: every session shares the
        // one map, and the claims model above the backend enforces
        // conflicts.
        let _ = id;
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
            return Err(VfsError::IsADirectory(path.to_string()));
        }
        Err(VfsError::NotFound(path.to_string()))
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
            return Err(VfsError::NotFound(path.to_string()));
        }
        if key == "/" {
            return Err(VfsError::PermissionDenied(
                "the namespace root cannot be removed".into(),
            ));
        }
        if tree.has_children(key) && !recursive {
            return Err(VfsError::DirectoryNotEmpty(path.to_string()));
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
        if pattern.len() > MAX_GLOB_PATTERN_BYTES {
            return Err(VfsError::InvalidPath(format!(
                "glob pattern exceeds {MAX_GLOB_PATTERN_BYTES} bytes"
            )));
        }
        if let Err(reason) = validate_glob_grammar(pattern) {
            return Err(VfsError::InvalidPath(format!(
                "invalid glob pattern {pattern:?}: {reason}"
            )));
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
            return Err(VfsError::NotADirectory(path.to_string()));
        }
        if !tree.dirs.contains(key) {
            return Err(VfsError::NotFound(path.to_string()));
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
            .ok_or_else(|| VfsError::NotFound(path.to_string()))
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let mut tree = self.tree();
        let key = path.as_str();
        if tree.contains(key) {
            return Err(VfsError::AlreadyExists(path.to_string()));
        }
        if let Some(file) = ancestors(key)
            .into_iter()
            .find(|ancestor| tree.files.contains_key(*ancestor))
        {
            return Err(VfsError::NotADirectory(file.to_owned()));
        }
        let missing_ancestors: Vec<&str> = ancestors(key)
            .into_iter()
            .filter(|ancestor| !tree.dirs.contains(*ancestor))
            .collect();
        if !recursive && !missing_ancestors.is_empty() {
            return Err(VfsError::NotFound(format!(
                "the parent of {path} does not exist"
            )));
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
            return Err(VfsError::PermissionDenied(
                "the namespace root cannot be renamed".into(),
            ));
        }
        if dest.starts_with(&format!("{source}/")) {
            return Err(VfsError::InvalidPath(format!(
                "cannot rename {source} into its own descendant {dest}"
            )));
        }
        if let Some(bytes) = tree.files.get(source).cloned() {
            tree.check_file_destination(dest)?;
            tree.create_ancestors(dest);
            tree.files.remove(source);
            tree.files.insert(dest.to_owned(), bytes);
            return Ok(());
        }
        if !tree.dirs.contains(source) {
            return Err(VfsError::NotFound(from.to_string()));
        }
        // A directory moves with its whole subtree. Validation finishes
        // before any mutation, so a failed rename changes nothing.
        if dest == "/" {
            return Err(VfsError::PermissionDenied(
                "a directory cannot be renamed onto the namespace root".into(),
            ));
        }
        if tree.files.contains_key(dest) {
            return Err(VfsError::NotADirectory(to.to_string()));
        }
        if tree.dirs.contains(dest) && tree.has_children(dest) {
            return Err(VfsError::DirectoryNotEmpty(to.to_string()));
        }
        if let Some(ancestor) = ancestors(dest)
            .into_iter()
            .find(|ancestor| tree.files.contains_key(*ancestor))
        {
            return Err(VfsError::NotADirectory(ancestor.to_owned()));
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
                return Err(VfsError::IsADirectory(from.to_string()));
            }
            return Err(VfsError::NotFound(from.to_string()));
        };
        tree.check_file_destination(to.as_str())?;
        tree.create_ancestors(to.as_str());
        tree.files.insert(to.to_string(), bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryBackend;
    use crate::error::VfsError;
    use crate::path::{VfsPath, canonicalize};
    use crate::traits::{ExecId, Vfs, VfsAccess};
    use crate::types::FileType;

    fn path(s: &str) -> Result<VfsPath, VfsError> {
        canonicalize(s)
    }

    /// Returns a session on a backend pre-populated through the write
    /// path, so seeding exercises the same code the tests do.
    fn seeded(files: &[(&str, &str)]) -> Result<Box<dyn VfsAccess>, VfsError> {
        let mut backend = MemoryBackend::new();
        let mut access = backend.acquire(ExecId::vend())?;
        for (name, text) in files {
            access.write(&path(name)?, text.as_bytes())?;
        }
        Ok(access)
    }

    #[test]
    fn read_returns_the_exact_bytes_stored() -> Result<(), VfsError> {
        let mut backend = MemoryBackend::new();
        let mut access = backend.acquire(ExecId::vend())?;
        let bytes = [0x00_u8, 0xff, 0x00, 0x7f];
        access.write(&path("/bin.dat")?, &bytes)?;
        assert_eq!(access.read(&path("/bin.dat")?)?, bytes);
        Ok(())
    }

    #[test]
    fn read_of_an_absent_path_is_not_found() -> Result<(), VfsError> {
        let access = seeded(&[])?;
        assert!(matches!(
            access.read(&path("/missing.txt")?),
            Err(VfsError::NotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn read_of_a_directory_is_is_a_directory() -> Result<(), VfsError> {
        let access = seeded(&[("/dir/f.txt", "x")])?;
        assert!(matches!(
            access.read(&path("/dir")?),
            Err(VfsError::IsADirectory(_))
        ));
        Ok(())
    }

    #[test]
    fn read_range_slices_bytes_and_clips_at_the_end() -> Result<(), VfsError> {
        let access = seeded(&[("/f.txt", "hello world")])?;
        assert_eq!(access.read_range(&path("/f.txt")?, 6, 5)?, b"world");
        assert_eq!(access.read_range(&path("/f.txt")?, 6, 100)?, b"world");
        assert!(access.read_range(&path("/f.txt")?, 100, 5)?.is_empty());
        Ok(())
    }

    #[test]
    fn write_creates_overwrites_and_materializes_ancestor_directories() -> Result<(), VfsError> {
        let mut access = seeded(&[])?;
        access.write(&path("/a/b/f.txt")?, b"one")?;
        assert_eq!(access.read(&path("/a/b/f.txt")?)?, b"one");
        // MemStore semantics: no mkdir was needed; the ancestors exist.
        assert!(access.exists(&path("/a")?)?);
        assert!(access.exists(&path("/a/b")?)?);
        access.write(&path("/a/b/f.txt")?, b"two")?;
        assert_eq!(access.read(&path("/a/b/f.txt")?)?, b"two");
        Ok(())
    }

    #[test]
    fn write_at_a_directory_path_is_rejected_without_touching_the_tree() -> Result<(), VfsError> {
        let mut access = seeded(&[("/dir/f.txt", "x")])?;
        assert!(matches!(
            access.write(&path("/dir")?, b"y"),
            Err(VfsError::IsADirectory(_))
        ));
        assert_eq!(access.read(&path("/dir/f.txt")?)?, b"x");
        Ok(())
    }

    #[test]
    fn append_creates_when_absent_and_extends_when_present() -> Result<(), VfsError> {
        let mut access = seeded(&[])?;
        access.append(&path("/log.txt")?, b"first\n")?;
        access.append(&path("/log.txt")?, b"second")?;
        assert_eq!(access.read(&path("/log.txt")?)?, b"first\nsecond");
        Ok(())
    }

    #[test]
    fn remove_of_an_absent_path_is_not_found() -> Result<(), VfsError> {
        let mut access = seeded(&[])?;
        assert!(matches!(
            access.remove(&path("/gone.txt")?, false),
            Err(VfsError::NotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn remove_of_a_file_removes_it() -> Result<(), VfsError> {
        let mut access = seeded(&[("/f.txt", "x")])?;
        access.remove(&path("/f.txt")?, false)?;
        assert!(!access.exists(&path("/f.txt")?)?);
        Ok(())
    }

    #[test]
    fn remove_of_a_nonempty_directory_without_recursive_is_an_error() -> Result<(), VfsError> {
        let mut access = seeded(&[("/dir/f.txt", "x")])?;
        assert!(matches!(
            access.remove(&path("/dir")?, false),
            Err(VfsError::DirectoryNotEmpty(_))
        ));
        // The failed removal changed nothing.
        assert_eq!(access.read(&path("/dir/f.txt")?)?, b"x");
        assert!(access.exists(&path("/dir")?)?);
        Ok(())
    }

    #[test]
    fn remove_of_an_empty_directory_without_recursive_succeeds() -> Result<(), VfsError> {
        let mut access = seeded(&[])?;
        access.mkdir(&path("/empty")?, false)?;
        access.remove(&path("/empty")?, false)?;
        assert!(!access.exists(&path("/empty")?)?);
        Ok(())
    }

    #[test]
    fn remove_with_recursive_deletes_the_whole_subtree() -> Result<(), VfsError> {
        let mut access = seeded(&[("/d/a.txt", "a"), ("/d/sub/b.txt", "b"), ("/keep.txt", "k")])?;
        access.remove(&path("/d")?, true)?;
        assert!(!access.exists(&path("/d")?)?);
        assert!(!access.exists(&path("/d/sub")?)?);
        assert!(!access.exists(&path("/d/sub/b.txt")?)?);
        assert_eq!(access.read(&path("/keep.txt")?)?, b"k");
        Ok(())
    }

    #[test]
    fn the_namespace_root_cannot_be_removed() -> Result<(), VfsError> {
        let mut access = seeded(&[("/f.txt", "x")])?;
        assert!(matches!(
            access.remove(&path("/")?, true),
            Err(VfsError::PermissionDenied(_))
        ));
        assert_eq!(access.read(&path("/f.txt")?)?, b"x");
        Ok(())
    }

    #[test]
    fn exists_distinguishes_files_directories_and_absence() -> Result<(), VfsError> {
        let access = seeded(&[("/dir/f.txt", "x")])?;
        assert!(access.exists(&path("/dir/f.txt")?)?);
        assert!(access.exists(&path("/dir")?)?);
        assert!(access.exists(&path("/")?)?);
        assert!(!access.exists(&path("/dir/missing.txt")?)?);
        Ok(())
    }

    #[test]
    fn glob_matches_star_within_a_segment_and_double_star_across() -> Result<(), VfsError> {
        let access = seeded(&[
            ("/src/a.rs", ""),
            ("/src/b.rs", ""),
            ("/src/deep/c.rs", ""),
            ("/notes/today.md", ""),
        ])?;
        assert_eq!(
            access.glob("/src/*.rs")?,
            vec!["/src/a.rs".to_owned(), "/src/b.rs".to_owned()]
        );
        assert_eq!(
            access.glob("/src/**/*.rs")?,
            vec![
                "/src/a.rs".to_owned(),
                "/src/b.rs".to_owned(),
                "/src/deep/c.rs".to_owned(),
            ]
        );
        assert_eq!(access.glob("/**/*.md")?, vec!["/notes/today.md".to_owned()]);
        Ok(())
    }

    #[test]
    fn glob_results_are_sorted_and_include_directories() -> Result<(), VfsError> {
        let access = seeded(&[("/d/b.txt", ""), ("/d/a.txt", ""), ("/d/sub/c.txt", "")])?;
        assert_eq!(
            access.glob("/d/*")?,
            vec![
                "/d/a.txt".to_owned(),
                "/d/b.txt".to_owned(),
                "/d/sub".to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn glob_rejects_invalid_patterns() -> Result<(), VfsError> {
        let access = seeded(&[("/f.txt", "x")])?;
        assert!(matches!(
            access.glob("/a/***/b"),
            Err(VfsError::InvalidPath(_))
        ));
        assert!(matches!(
            access.glob("/a\\b"),
            Err(VfsError::InvalidPath(_))
        ));
        Ok(())
    }

    #[test]
    fn list_returns_sorted_entries_with_stats() -> Result<(), VfsError> {
        let access = seeded(&[("/d/b.txt", "bb"), ("/d/a.txt", "a"), ("/d/sub/c.txt", "c")])?;
        let entries = access.list(&path("/d")?)?;
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt", "sub"]);
        assert_eq!(entries[0].stat.file_type, FileType::File);
        assert_eq!(entries[0].stat.size, 1);
        assert_eq!(entries[2].stat.file_type, FileType::Directory);
        assert!(entries.iter().all(|entry| entry.description.is_none()));
        Ok(())
    }

    #[test]
    fn list_of_a_file_or_an_absent_path_is_an_error() -> Result<(), VfsError> {
        let access = seeded(&[("/f.txt", "x")])?;
        assert!(matches!(
            access.list(&path("/f.txt")?),
            Err(VfsError::NotADirectory(_))
        ));
        assert!(matches!(
            access.list(&path("/missing")?),
            Err(VfsError::NotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn stat_reports_kinds_and_sizes_without_fabricated_times() -> Result<(), VfsError> {
        let access = seeded(&[("/dir/f.txt", "hello")])?;
        let file = access.stat(&path("/dir/f.txt")?)?;
        assert_eq!(file.file_type, FileType::File);
        assert_eq!(file.size, 5);
        assert!(file.mode.is_none() && file.modified.is_none() && file.created.is_none());
        let dir = access.stat(&path("/dir")?)?;
        assert_eq!(dir.file_type, FileType::Directory);
        assert!(matches!(
            access.stat(&path("/missing")?),
            Err(VfsError::NotFound(_))
        ));
        Ok(())
    }

    #[test]
    fn mkdir_creates_directories_and_rejects_existing_paths() -> Result<(), VfsError> {
        let mut access = seeded(&[("/f.txt", "x")])?;
        access.mkdir(&path("/new")?, false)?;
        assert!(access.exists(&path("/new")?)?);
        assert!(matches!(
            access.mkdir(&path("/new")?, false),
            Err(VfsError::AlreadyExists(_))
        ));
        assert!(matches!(
            access.mkdir(&path("/f.txt")?, false),
            Err(VfsError::AlreadyExists(_))
        ));
        Ok(())
    }

    #[test]
    fn mkdir_without_recursive_requires_an_existing_parent() -> Result<(), VfsError> {
        let mut access = seeded(&[])?;
        assert!(matches!(
            access.mkdir(&path("/a/b")?, false),
            Err(VfsError::NotFound(_))
        ));
        access.mkdir(&path("/a/b")?, true)?;
        assert!(access.exists(&path("/a")?)?);
        assert!(access.exists(&path("/a/b")?)?);
        Ok(())
    }

    #[test]
    fn mkdir_through_a_file_parent_is_not_a_directory() -> Result<(), VfsError> {
        let mut access = seeded(&[("/f.txt", "x")])?;
        assert!(matches!(
            access.mkdir(&path("/f.txt/g")?, true),
            Err(VfsError::NotADirectory(_))
        ));
        Ok(())
    }

    #[test]
    fn rename_moves_a_file_and_leaves_nothing_behind() -> Result<(), VfsError> {
        let mut access = seeded(&[("/from.txt", "data")])?;
        access.rename(&path("/from.txt")?, &path("/sub/to.txt")?)?;
        assert!(!access.exists(&path("/from.txt")?)?);
        assert_eq!(access.read(&path("/sub/to.txt")?)?, b"data");
        Ok(())
    }

    #[test]
    fn rename_moves_a_directory_subtree() -> Result<(), VfsError> {
        let mut access = seeded(&[("/d/a.txt", "a"), ("/d/sub/b.txt", "b")])?;
        access.rename(&path("/d")?, &path("/moved")?)?;
        assert!(!access.exists(&path("/d")?)?);
        assert_eq!(access.read(&path("/moved/a.txt")?)?, b"a");
        assert_eq!(access.read(&path("/moved/sub/b.txt")?)?, b"b");
        Ok(())
    }

    #[test]
    fn renaming_a_directory_onto_the_root_is_rejected() -> Result<(), VfsError> {
        let mut access = seeded(&[("/d/a.txt", "a"), ("/other.txt", "o")])?;
        assert!(matches!(
            access.rename(&path("/d")?, &path("/")?),
            Err(VfsError::PermissionDenied(_))
        ));
        // The failed rename changed nothing: the subtree is intact.
        assert_eq!(access.read(&path("/d/a.txt")?)?, b"a");
        assert_eq!(access.read(&path("/other.txt")?)?, b"o");
        let root: Vec<String> = access
            .list(&path("/")?)?
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(root, vec!["d".to_owned(), "other.txt".to_owned()]);
        Ok(())
    }

    #[test]
    fn a_failed_rename_leaves_source_and_destination_unchanged() -> Result<(), VfsError> {
        let mut access = seeded(&[("/dst.txt", "old")])?;
        assert!(matches!(
            access.rename(&path("/missing.txt")?, &path("/dst.txt")?),
            Err(VfsError::NotFound(_))
        ));
        assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
        // Renaming a directory into its own descendant is rejected.
        access.mkdir(&path("/d")?, false)?;
        assert!(matches!(
            access.rename(&path("/d")?, &path("/d/inner")?),
            Err(VfsError::InvalidPath(_))
        ));
        assert!(access.exists(&path("/d")?)?);
        Ok(())
    }

    #[test]
    fn copy_duplicates_a_files_bytes() -> Result<(), VfsError> {
        let mut access = seeded(&[("/src.txt", "data")])?;
        access.copy(&path("/src.txt")?, &path("/dst.txt")?)?;
        assert_eq!(access.read(&path("/src.txt")?)?, b"data");
        assert_eq!(access.read(&path("/dst.txt")?)?, b"data");
        Ok(())
    }

    #[test]
    fn copy_rejects_directories_and_a_failed_copy_changes_nothing() -> Result<(), VfsError> {
        let mut access = seeded(&[("/d/f.txt", "x"), ("/dst.txt", "old")])?;
        assert!(matches!(
            access.copy(&path("/d")?, &path("/dst.txt")?),
            Err(VfsError::IsADirectory(_))
        ));
        assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
        assert!(matches!(
            access.copy(&path("/missing.txt")?, &path("/dst.txt")?),
            Err(VfsError::NotFound(_))
        ));
        assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
        Ok(())
    }

    #[test]
    fn acquire_and_release_accept_attribution_as_a_no_op() -> Result<(), VfsError> {
        let mut backend = MemoryBackend::new();
        let mut first = backend.acquire(ExecId::vend())?;
        first.write(&path("/f.txt")?, b"shared")?;
        // A second identity's session sees the same map.
        let second = backend.acquire(ExecId::vend())?;
        assert_eq!(second.read(&path("/f.txt")?)?, b"shared");
        drop(first);
        drop(second);
        backend.release(ExecId::vend())?;
        Ok(())
    }

    #[test]
    fn the_default_is_a_meaningful_empty_backend() -> Result<(), VfsError> {
        let mut backend = MemoryBackend::default();
        let access = backend.acquire(ExecId::vend())?;
        assert!(access.exists(&path("/")?)?);
        assert!(!access.exists(&path("/anything")?)?);
        assert!(access.list(&path("/")?)?.is_empty());
        Ok(())
    }
}
