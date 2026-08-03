//! Run-scoped virtual files, shared by Lua and the model.
//!
//! A prompt run keeps its bulk state in virtual files addressed by logical
//! string paths. [`FileStore`] is the backend contract, [`MemVfs`] is an
//! in-memory backend, and [`Store`] is the cheaply cloneable, thread-safe
//! handle the runtime hands to both the Lua VM and (later) the model's file
//! tools. Reads return numbered lines for navigation and error messages, and
//! edits are anchor-based ([`FileStore::str_replace`]) rather than offset-based,
//! the shape that works for a model.
//!
//! This module wires no execution; it defines the store and its in-memory
//! backend only.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// An error from a virtual-file operation.
///
/// Marked `#[non_exhaustive]` so a future backend (real filesystem, network)
/// can add variants without a breaking change; each data-carrying variant is
/// likewise `#[non_exhaustive]`. `Display` messages are lowercase noun phrases
/// with no trailing period, so a caller supplies the surrounding context.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// No file exists at the requested path.
    #[error("file not found: {path}")]
    #[non_exhaustive]
    NotFound {
        /// The logical path that did not resolve to a file.
        path: String,
    },

    /// The `str_replace` anchor did not occur in the file.
    #[error("anchor not found in {path}")]
    #[non_exhaustive]
    AnchorNotFound {
        /// The logical path that was searched.
        path: String,
        /// The anchor text that was not found.
        anchor: String,
    },

    /// The `str_replace` anchor occurred more than once, so the edit is
    /// ambiguous and is refused rather than applied to an arbitrary match.
    #[error("anchor occurs {count} times in {path}, expected exactly one")]
    #[non_exhaustive]
    AnchorAmbiguous {
        /// The logical path that was searched.
        path: String,
        /// The anchor text that matched more than once.
        anchor: String,
        /// The number of times the anchor matched.
        count: usize,
    },
}

/// A backend for run-scoped virtual files addressed by logical string paths.
///
/// All operations are synchronous. Implementors store text keyed by path; the
/// runtime shares one behind a [`Store`] handle. Reads render numbered lines
/// (see [`FileStore::read`]) and edits are anchored (see
/// [`FileStore::str_replace`]).
///
/// # Examples
/// ```
/// use promptforge_core::store::{FileStore, MemVfs};
///
/// let mut fs = MemVfs::new();
/// fs.write("greeting.txt", "hello")?;
/// assert_eq!(fs.read("greeting.txt")?, "1| hello");
/// # Ok::<(), promptforge_core::store::StoreError>(())
/// ```
pub trait FileStore {
    /// Creates the file at `path`, or overwrites it if it already exists.
    ///
    /// # Errors
    /// This operation does not fail for the in-memory backend, but the return
    /// type is fallible so a filesystem-backed backend can report I/O errors.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{FileStore, MemVfs};
    ///
    /// let mut fs = MemVfs::new();
    /// fs.write("a.txt", "one")?;
    /// fs.write("a.txt", "two")?;
    /// assert_eq!(fs.read("a.txt")?, "1| two");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    fn write(&mut self, path: &str, contents: &str) -> Result<(), StoreError>;

    /// Appends `contents` to the file at `path`, creating it if it is absent.
    ///
    /// # Errors
    /// This operation does not fail for the in-memory backend; the return type
    /// stays fallible for filesystem-backed backends.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{FileStore, MemVfs};
    ///
    /// let mut fs = MemVfs::new();
    /// fs.append("log.txt", "first\n")?;
    /// fs.append("log.txt", "second")?;
    /// assert_eq!(fs.read("log.txt")?, "1| first\n2| second");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    fn append(&mut self, path: &str, contents: &str) -> Result<(), StoreError>;

    /// Returns the file's contents as numbered lines.
    ///
    /// Each line is prefixed with its 1-based number, right-aligned to the
    /// width of the highest number, followed by `"| "`; lines are joined with
    /// `"\n"` and there is no trailing newline. An empty file reads as the
    /// empty string. The numbering is for navigation and error messages, not a
    /// wire format.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{FileStore, MemVfs};
    ///
    /// let mut fs = MemVfs::new();
    /// fs.write("poem.txt", "roses\nviolets")?;
    /// assert_eq!(fs.read("poem.txt")?, "1| roses\n2| violets");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    fn read(&self, path: &str) -> Result<String, StoreError>;

    /// Replaces the single occurrence of `old` with `new` in the file at
    /// `path`.
    ///
    /// The edit is anchor-based: `old` must occur exactly once. Zero matches
    /// and more-than-one match are both refused, so an edit never lands on an
    /// arbitrary match.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`,
    /// [`StoreError::AnchorNotFound`] if `old` does not occur, or
    /// [`StoreError::AnchorAmbiguous`] if `old` occurs more than once.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{FileStore, MemVfs};
    ///
    /// let mut fs = MemVfs::new();
    /// fs.write("a.txt", "the quick brown fox")?;
    /// fs.str_replace("a.txt", "quick", "slow")?;
    /// assert_eq!(fs.read("a.txt")?, "1| the slow brown fox");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    fn str_replace(&mut self, path: &str, old: &str, new: &str) -> Result<(), StoreError>;

    /// Removes the file at `path`.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{FileStore, MemVfs};
    ///
    /// let mut fs = MemVfs::new();
    /// fs.write("temp.txt", "scratch")?;
    /// fs.delete("temp.txt")?;
    /// assert!(fs.read("temp.txt").is_err());
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    fn delete(&mut self, path: &str) -> Result<(), StoreError>;

    /// Returns every stored path matching `pattern`, in sorted order.
    ///
    /// Two wildcards are supported: `*` matches any run of characters within a
    /// single path segment (it never crosses `/`), and `**` matches any run of
    /// characters including `/`. All other characters match literally.
    ///
    /// # Errors
    /// This operation does not fail for the in-memory backend; the return type
    /// stays fallible for filesystem-backed backends.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{FileStore, MemVfs};
    ///
    /// let mut fs = MemVfs::new();
    /// fs.write("src/a.rs", "")?;
    /// fs.write("src/b.rs", "")?;
    /// fs.write("src/deep/c.rs", "")?;
    /// assert_eq!(fs.glob("src/*.rs")?, vec!["src/a.rs", "src/b.rs"]);
    /// assert_eq!(
    ///     fs.glob("src/**/*.rs")?,
    ///     vec!["src/a.rs", "src/b.rs", "src/deep/c.rs"],
    /// );
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    fn glob(&self, pattern: &str) -> Result<Vec<String>, StoreError>;
}

/// An in-memory [`FileStore`] backend.
///
/// Files live in a [`BTreeMap`] keyed by path, so listing and [`glob`] results
/// are ordered without a sort step. It holds no resources and drops with the
/// run.
///
/// [`glob`]: FileStore::glob
///
/// # Examples
/// ```
/// use promptforge_core::store::{FileStore, MemVfs};
///
/// let mut fs = MemVfs::new();
/// fs.write("notes.md", "todo")?;
/// assert_eq!(fs.glob("*.md")?, vec!["notes.md"]);
/// # Ok::<(), promptforge_core::store::StoreError>(())
/// ```
#[derive(Debug, Default, Clone)]
pub struct MemVfs {
    files: BTreeMap<String, String>,
}

impl MemVfs {
    /// Creates an empty in-memory store.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::MemVfs;
    ///
    /// let fs = MemVfs::new();
    /// # let _ = fs;
    /// ```
    #[must_use]
    pub fn new() -> MemVfs {
        MemVfs::default()
    }
}

impl FileStore for MemVfs {
    fn write(&mut self, path: &str, contents: &str) -> Result<(), StoreError> {
        self.files.insert(path.to_string(), contents.to_string());
        Ok(())
    }

    fn append(&mut self, path: &str, contents: &str) -> Result<(), StoreError> {
        self.files
            .entry(path.to_string())
            .or_default()
            .push_str(contents);
        Ok(())
    }

    fn read(&self, path: &str) -> Result<String, StoreError> {
        let contents = self.files.get(path).ok_or_else(|| StoreError::NotFound {
            path: path.to_string(),
        })?;
        Ok(number_lines(contents))
    }

    fn str_replace(&mut self, path: &str, old: &str, new: &str) -> Result<(), StoreError> {
        let contents = self.files.get(path).ok_or_else(|| StoreError::NotFound {
            path: path.to_string(),
        })?;
        let count = contents.matches(old).count();
        match count {
            0 => Err(StoreError::AnchorNotFound {
                path: path.to_string(),
                anchor: old.to_string(),
            }),
            1 => {
                let replaced = contents.replacen(old, new, 1);
                self.files.insert(path.to_string(), replaced);
                Ok(())
            }
            count => Err(StoreError::AnchorAmbiguous {
                path: path.to_string(),
                anchor: old.to_string(),
                count,
            }),
        }
    }

    fn delete(&mut self, path: &str) -> Result<(), StoreError> {
        if self.files.remove(path).is_some() {
            Ok(())
        } else {
            Err(StoreError::NotFound {
                path: path.to_string(),
            })
        }
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, StoreError> {
        Ok(self
            .files
            .keys()
            .filter(|key| glob_match(pattern.as_bytes(), key.as_bytes()))
            .cloned()
            .collect())
    }
}

/// A cheaply cloneable, thread-safe handle to a run's virtual files.
///
/// The handle wraps `Arc<Mutex<Box<dyn FileStore + Send + Sync>>>`, so cloning
/// shares one backend and the store can be held by both the synchronous Lua VM
/// and an asynchronous tool whose `call` crosses an `.await`. The inherent
/// methods mirror [`FileStore`], each taking the lock, delegating, and
/// releasing it before returning; no lock is ever held across an await, and the
/// operations are synchronous in any case.
///
/// # Examples
/// ```
/// use promptforge_core::store::Store;
///
/// let store = Store::memory();
/// let clone = store.clone();
/// store.write("shared.txt", "state")?;
/// assert_eq!(clone.read("shared.txt")?, "1| state");
/// # Ok::<(), promptforge_core::store::StoreError>(())
/// ```
#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Box<dyn FileStore + Send + Sync>>>,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Wraps `backend` in a shareable handle.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{MemVfs, Store};
    ///
    /// let store = Store::new(Box::new(MemVfs::new()));
    /// # let _ = store;
    /// ```
    #[must_use]
    pub fn new(backend: Box<dyn FileStore + Send + Sync>) -> Store {
        Store {
            inner: Arc::new(Mutex::new(backend)),
        }
    }

    /// Builds a handle over a fresh in-memory [`MemVfs`] backend.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::Store;
    ///
    /// let store = Store::memory();
    /// # let _ = store;
    /// ```
    #[must_use]
    pub fn memory() -> Store {
        Store::new(Box::new(MemVfs::new()))
    }

    /// Recovers the guard even if a prior holder panicked; the stored map stays
    /// consistent, so poisoning is not a fatal condition here.
    fn lock(&self) -> MutexGuard<'_, Box<dyn FileStore + Send + Sync>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Creates or overwrites the file at `path`. See [`FileStore::write`].
    ///
    /// # Errors
    /// Propagates any [`StoreError`] from the backend.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::Store;
    ///
    /// let store = Store::memory();
    /// store.write("a.txt", "hi")?;
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn write(&self, path: &str, contents: &str) -> Result<(), StoreError> {
        self.lock().write(path, contents)
    }

    /// Appends to the file at `path`, creating it if absent. See
    /// [`FileStore::append`].
    ///
    /// # Errors
    /// Propagates any [`StoreError`] from the backend.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::Store;
    ///
    /// let store = Store::memory();
    /// store.append("a.txt", "hi")?;
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn append(&self, path: &str, contents: &str) -> Result<(), StoreError> {
        self.lock().append(path, contents)
    }

    /// Reads the file at `path` as numbered lines. See [`FileStore::read`].
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::Store;
    ///
    /// let store = Store::memory();
    /// store.write("a.txt", "hi")?;
    /// assert_eq!(store.read("a.txt")?, "1| hi");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn read(&self, path: &str) -> Result<String, StoreError> {
        self.lock().read(path)
    }

    /// Replaces the unique occurrence of `old` with `new`. See
    /// [`FileStore::str_replace`].
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`], [`StoreError::AnchorNotFound`], or
    /// [`StoreError::AnchorAmbiguous`] per [`FileStore::str_replace`].
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::Store;
    ///
    /// let store = Store::memory();
    /// store.write("a.txt", "one two")?;
    /// store.str_replace("a.txt", "two", "three")?;
    /// assert_eq!(store.read("a.txt")?, "1| one three");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn str_replace(&self, path: &str, old: &str, new: &str) -> Result<(), StoreError> {
        self.lock().str_replace(path, old, new)
    }

    /// Removes the file at `path`. See [`FileStore::delete`].
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::Store;
    ///
    /// let store = Store::memory();
    /// store.write("a.txt", "hi")?;
    /// store.delete("a.txt")?;
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn delete(&self, path: &str) -> Result<(), StoreError> {
        self.lock().delete(path)
    }

    /// Returns stored paths matching `pattern`, sorted. See [`FileStore::glob`].
    ///
    /// # Errors
    /// Propagates any [`StoreError`] from the backend.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::Store;
    ///
    /// let store = Store::memory();
    /// store.write("a.txt", "")?;
    /// store.write("b.md", "")?;
    /// assert_eq!(store.glob("*.txt")?, vec!["a.txt"]);
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn glob(&self, pattern: &str) -> Result<Vec<String>, StoreError> {
        self.lock().glob(pattern)
    }
}

/// Renders `content` as numbered lines, right-aligned to the widest number.
fn number_lines(content: &str) -> String {
    let total = content.lines().count();
    if total == 0 {
        return String::new();
    }
    let width = total.to_string().len();
    let mut out = String::new();
    for (index, line) in content.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let number = index + 1;
        // Writing to a String is infallible; the result carries no information.
        let _ = write!(out, "{number:>width$}| {line}");
    }
    out
}

/// Matches `text` against a glob `pattern` where `*` stays within a segment and
/// `**` spans `/`.
fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let Some((&first, pattern_rest)) = pattern.split_first() else {
        return text.is_empty();
    };
    if first == b'*' {
        if let Some((&b'*', double_rest)) = pattern_rest.split_first() {
            // `**/` also matches zero path segments, so `a/**/b` matches `a/b`.
            if let Some((&b'/', after_slash)) = double_rest.split_first()
                && glob_match(after_slash, text)
            {
                return true;
            }
            return (0..=text.len()).any(|skip| glob_match(double_rest, &text[skip..]));
        }
        let mut cursor = 0;
        loop {
            if glob_match(pattern_rest, &text[cursor..]) {
                return true;
            }
            match text.get(cursor) {
                Some(&byte) if byte != b'/' => cursor += 1,
                _ => return false,
            }
        }
    }
    match text.split_first() {
        Some((&head, text_rest)) if head == first => glob_match(pattern_rest, text_rest),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_numbers_lines() {
        let store = Store::memory();
        store.write("a.txt", "first\nsecond\nthird").expect("write");
        assert_eq!(
            store.read("a.txt").expect("read"),
            "1| first\n2| second\n3| third"
        );
    }

    #[test]
    fn read_pads_numbers_to_width() {
        let store = Store::memory();
        let mut body = String::new();
        for n in 1..=10 {
            use std::fmt::Write as _;
            let _ = writeln!(body, "line{n}");
        }
        store.write("a.txt", &body).expect("write");
        let numbered = store.read("a.txt").expect("read");
        assert!(numbered.starts_with(" 1| line1\n"));
        assert!(numbered.contains("\n10| line10"));
    }

    #[test]
    fn write_overwrites() {
        let store = Store::memory();
        store.write("a.txt", "old").expect("write");
        store.write("a.txt", "new").expect("overwrite");
        assert_eq!(store.read("a.txt").expect("read"), "1| new");
    }

    #[test]
    fn read_empty_file_is_empty_string() {
        let store = Store::memory();
        store.write("e.txt", "").expect("write");
        assert_eq!(store.read("e.txt").expect("read"), "");
    }

    #[test]
    fn append_creates_then_extends() {
        let store = Store::memory();
        store.append("log.txt", "one\n").expect("create via append");
        store.append("log.txt", "two").expect("extend");
        assert_eq!(store.read("log.txt").expect("read"), "1| one\n2| two");
    }

    #[test]
    fn str_replace_replaces_unique() {
        let store = Store::memory();
        store.write("a.txt", "the quick brown fox").expect("write");
        store
            .str_replace("a.txt", "quick", "slow")
            .expect("replace");
        assert_eq!(store.read("a.txt").expect("read"), "1| the slow brown fox");
    }

    #[test]
    fn str_replace_missing_anchor_errors() {
        let store = Store::memory();
        store.write("a.txt", "hello world").expect("write");
        let err = store
            .str_replace("a.txt", "absent", "x")
            .expect_err("should fail");
        assert!(matches!(err, StoreError::AnchorNotFound { .. }));
    }

    #[test]
    fn str_replace_ambiguous_anchor_errors() {
        let store = Store::memory();
        store.write("a.txt", "na na na").expect("write");
        let err = store
            .str_replace("a.txt", "na", "la")
            .expect_err("should fail");
        match err {
            StoreError::AnchorAmbiguous { count, .. } => assert_eq!(count, 3),
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn str_replace_on_missing_file_errors() {
        let store = Store::memory();
        let err = store
            .str_replace("nope.txt", "a", "b")
            .expect_err("should fail");
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[test]
    fn delete_then_read_errors() {
        let store = Store::memory();
        store.write("a.txt", "gone soon").expect("write");
        store.delete("a.txt").expect("delete");
        let err = store.read("a.txt").expect_err("should fail");
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[test]
    fn delete_missing_errors() {
        let store = Store::memory();
        let err = store.delete("absent.txt").expect_err("should fail");
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[test]
    fn glob_matches_sorted() {
        let store = Store::memory();
        for path in ["src/b.rs", "src/a.rs", "src/deep/c.rs", "notes.md"] {
            store.write(path, "").expect("write");
        }
        assert_eq!(
            store.glob("src/*.rs").expect("glob"),
            vec!["src/a.rs", "src/b.rs"]
        );
        assert_eq!(
            store.glob("src/**/*.rs").expect("glob"),
            vec!["src/a.rs", "src/b.rs", "src/deep/c.rs"],
        );
        assert_eq!(store.glob("*.md").expect("glob"), vec!["notes.md"]);
        assert_eq!(store.glob("**").expect("glob").len(), 4);
    }

    #[test]
    fn glob_star_stops_at_slash() {
        let store = Store::memory();
        store.write("a/b.txt", "").expect("write");
        assert!(store.glob("*.txt").expect("glob").is_empty());
        assert_eq!(store.glob("a/*.txt").expect("glob"), vec!["a/b.txt"]);
    }

    #[test]
    fn clones_share_backing_state() {
        let store = Store::memory();
        let clone = store.clone();
        store
            .write("shared.txt", "written by original")
            .expect("write");
        assert_eq!(
            clone.read("shared.txt").expect("read"),
            "1| written by original"
        );
        clone
            .write("second.txt", "written by clone")
            .expect("write");
        assert_eq!(
            store.read("second.txt").expect("read"),
            "1| written by clone"
        );
    }
}
