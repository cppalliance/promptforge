//! The [`Store`] backend contract and its in-memory implementation.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::StoreError;
use super::glob::{compile_glob, matches_tokens};
use super::path::StorePath;

/// A backend for run-scoped virtual files addressed by logical string paths.
///
/// All operations are synchronous. Implementors store text keyed by path; the
/// runtime shares one behind a [`StoreRef`](super::StoreRef) handle. Numbered
/// reads use [`Store::read_lines`]; verbatim reads use [`Store::read`]; edits
/// are anchored (see [`Store::str_replace`]).
///
/// # Examples
/// ```
/// use promptforge_core::store::{Store, MemStore};
///
/// let mut fs = MemStore::new();
/// fs.write("greeting.txt", "hello")?;
/// assert_eq!(fs.read_lines("greeting.txt")?, "1| hello");
/// assert_eq!(fs.read("greeting.txt")?, "hello");
/// # Ok::<(), promptforge_core::store::StoreError>(())
/// ```
///
/// The `Send` bound lets a backend cross a `spawn_blocking` boundary; `Sync` is
/// deliberately not required, since the runtime serializes access behind a
/// [`StoreRef`](super::StoreRef) mutex.
pub trait Store: Send {
    /// Creates the file at `path`, or overwrites it if it already exists.
    ///
    /// # Errors
    /// This operation does not fail for the in-memory backend, but the return
    /// type is fallible so a filesystem-backed backend can report I/O errors.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{Store, MemStore};
    ///
    /// let mut fs = MemStore::new();
    /// fs.write("a.txt", "one")?;
    /// fs.write("a.txt", "two")?;
    /// assert_eq!(fs.read_lines("a.txt")?, "1| two");
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
    /// use promptforge_core::store::{Store, MemStore};
    ///
    /// let mut fs = MemStore::new();
    /// fs.append("log.txt", "first\n")?;
    /// fs.append("log.txt", "second")?;
    /// assert_eq!(fs.read_lines("log.txt")?, "1| first\n2| second");
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
    /// use promptforge_core::store::{Store, MemStore};
    ///
    /// let mut fs = MemStore::new();
    /// fs.write("poem.txt", "roses\nviolets")?;
    /// assert_eq!(fs.read_lines("poem.txt")?, "1| roses\n2| violets");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    fn read_lines(&self, path: &str) -> Result<String, StoreError>;

    /// Returns the file's contents exactly as stored, with no line numbering.
    ///
    /// This is the accessor for verbatim handoff, clean dumps, and trusted
    /// re-injection. Use [`Store::read_lines`] when numbered output is
    /// needed for navigation.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{Store, MemStore};
    ///
    /// let mut fs = MemStore::new();
    /// fs.write("poem.txt", "roses\nviolets\n")?;
    /// assert_eq!(fs.read("poem.txt")?, "roses\nviolets\n");
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
    /// use promptforge_core::store::{Store, MemStore};
    ///
    /// let mut fs = MemStore::new();
    /// fs.write("a.txt", "the quick brown fox")?;
    /// fs.str_replace("a.txt", "quick", "slow")?;
    /// assert_eq!(fs.read_lines("a.txt")?, "1| the slow brown fox");
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
    /// use promptforge_core::store::{Store, MemStore};
    ///
    /// let mut fs = MemStore::new();
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
    /// use promptforge_core::store::{Store, MemStore};
    ///
    /// let mut fs = MemStore::new();
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

    /// Returns whether a file exists at `path`.
    ///
    /// This is fallible so a backend distinguishes a confirmed absence
    /// (`Ok(false)`) from an inability to answer (`Err`), rather than
    /// collapsing a backend failure into "does not exist".
    ///
    /// # Errors
    /// Returns a [`StoreError`] if the backend cannot determine existence.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{Store, MemStore};
    ///
    /// let mut fs = MemStore::new();
    /// assert!(!fs.exists("a.txt")?);
    /// fs.write("a.txt", "hi")?;
    /// assert!(fs.exists("a.txt")?);
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    fn exists(&self, path: &str) -> Result<bool, StoreError>;
}

/// An in-memory [`Store`] backend.
///
/// Files live in a [`BTreeMap`] keyed by path, so listing and [`glob`] results
/// are ordered without a sort step. It holds no resources and drops with the
/// run.
///
/// [`glob`]: Store::glob
///
/// # Examples
/// ```
/// use promptforge_core::store::{Store, MemStore};
///
/// let mut fs = MemStore::new();
/// fs.write("notes.md", "todo")?;
/// assert_eq!(fs.glob("*.md")?, vec!["notes.md"]);
/// # Ok::<(), promptforge_core::store::StoreError>(())
/// ```
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct MemStore {
    files: BTreeMap<String, String>,
}

impl MemStore {
    /// Creates an empty in-memory store.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::MemStore;
    ///
    /// let fs = MemStore::new();
    /// # let _ = fs;
    /// ```
    #[must_use]
    pub fn new() -> MemStore {
        MemStore::default()
    }

    /// Creates a store pre-populated with the given files.
    ///
    /// Each path is validated through `StorePath::parse` at
    /// construction time, so the store never holds a path unreachable through
    /// the normal read/write API.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidPath`] if any path fails validation.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{MemStore, Store};
    ///
    /// let fs = MemStore::with_files([
    ///     ("input.md".to_owned(), "# Hello".to_owned()),
    /// ])?;
    /// assert_eq!(fs.read("input.md")?, "# Hello");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn with_files(files: impl IntoIterator<Item = (String, String)>) -> Result<MemStore, StoreError> {
        let mut map = BTreeMap::new();
        for (path, contents) in files {
            let validated = StorePath::parse(&path)?;
            map.insert(validated.as_str().to_owned(), contents);
        }
        Ok(MemStore { files: map })
    }
}

impl Store for MemStore {
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

    fn read_lines(&self, path: &str) -> Result<String, StoreError> {
        let contents = self.files.get(path).ok_or_else(|| StoreError::NotFound {
            path: path.to_string(),
        })?;
        Ok(number_lines(contents))
    }

    fn read(&self, path: &str) -> Result<String, StoreError> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| StoreError::NotFound {
                path: path.to_string(),
            })
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
        // STORE-020: compile the pattern once, then reuse the tokens across every
        // key, so the per-key tokenization no longer repeats while the shared
        // store mutex is held. Matching itself is bounded and non-backtracking.
        let tokens = compile_glob(pattern.as_bytes());
        Ok(self
            .files
            .keys()
            .filter(|key| matches_tokens(&tokens, key.as_bytes()))
            .cloned()
            .collect())
    }

    fn exists(&self, path: &str) -> Result<bool, StoreError> {
        Ok(self.files.contains_key(path))
    }
}

/// Renders `content` as numbered lines, right-aligned to the widest number.
pub(super) fn number_lines(content: &str) -> String {
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
