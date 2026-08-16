//! Run-scoped virtual files, shared by Lua and the model.
//!
//! A prompt run keeps its bulk state in virtual files addressed by logical
//! string paths. [`Store`] is the backend contract, [`MemStore`] is an
//! in-memory backend, and [`StoreRef`] is the cheaply cloneable, thread-safe
//! handle the runtime hands to both the Lua VM and (later) the model's file
//! tools. Three read shapes are available: [`StoreRef::read_lines`] returns
//! numbered lines for navigation, [`StoreRef::read`] returns verbatim
//! contents for trusted handoff, and [`StoreRef::inject`] wraps verbatim
//! contents in an untrusted guard envelope for model-facing re-injection.
//! Edits are anchor-based ([`Store::str_replace`]) rather than offset-based,
//! the shape that works for a model.
//!
//! This module wires no execution; it defines the store and its in-memory
//! backend only.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

mod error;
mod file;
mod glob;
mod mem;
mod path;

use error::StorePoisoned;
pub use error::{PathReason, StoreError, StoreErrorKind};
pub use file::FileStore;
use glob::{MAX_GLOB_PATTERN_BYTES, compile_glob, matches_tokens, validate_glob_grammar};
pub use mem::{MemStore, Store};
use path::StorePath;

/// A cheaply cloneable, thread-safe handle to a run's virtual files.
///
/// The handle wraps `Arc<Mutex<Box<dyn Store + Send>>>`: the `Mutex` supplies
/// the synchronization around a `Send` (not necessarily `Sync`) backend
/// (STORE-008), so cloning shares one backend and the store can be held by both
/// the synchronous Lua VM and an asynchronous tool whose `call` crosses an
/// `.await`. The inherent
/// methods mirror [`Store`], each taking the lock, delegating, and
/// releasing it before returning; no lock is ever held across an await, and the
/// operations are synchronous in any case.
///
/// # Examples
/// ```
/// use promptforge_core::store::StoreRef;
///
/// let store = StoreRef::memory();
/// let clone = store.clone();
/// store.write("shared.txt", "state")?;
/// assert_eq!(clone.read_lines("shared.txt")?, "1| state");
/// assert_eq!(clone.read("shared.txt")?, "state");
/// # Ok::<(), promptforge_core::store::StoreError>(())
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct StoreRef {
    inner: Arc<Mutex<Box<dyn Store + Send>>>,
}

impl fmt::Debug for StoreRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreRef").finish_non_exhaustive()
    }
}

impl StoreRef {
    /// Wraps `backend` in a shareable handle.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::{MemStore, StoreRef};
    ///
    /// let store = StoreRef::new(Box::new(MemStore::new()));
    /// # let _ = store;
    /// ```
    #[must_use]
    pub fn new(backend: Box<dyn Store + Send>) -> StoreRef {
        StoreRef {
            inner: Arc::new(Mutex::new(backend)),
        }
    }

    /// Builds a handle over a [`MemStore`] pre-populated with the given files.
    ///
    /// Each path is validated at construction time. See
    /// [`MemStore::with_files`] for details.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidPath`] if any path fails validation.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::with_files([
    ///     ("data.txt".to_owned(), "contents".to_owned()),
    /// ])?;
    /// assert_eq!(store.read("data.txt")?, "contents");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn with_files(files: impl IntoIterator<Item = (String, String)>) -> Result<StoreRef, StoreError> {
        Ok(StoreRef::new(Box::new(MemStore::with_files(files)?)))
    }

    /// Builds a handle over a fresh in-memory [`MemStore`] backend.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// # let _ = store;
    /// ```
    #[must_use]
    pub fn memory() -> StoreRef {
        StoreRef::new(Box::new(MemStore::new()))
    }

    /// Locks the shared backend, or reports it unavailable if a prior holder
    /// panicked while mutating it.
    ///
    /// STORE-004: the backend behind this handle is an arbitrary [`Store`] trait
    /// object, not a known-consistent [`MemStore`]. A panic mid-mutation can
    /// leave a filesystem/network backend in a half-applied state, so we do NOT
    /// blindly `PoisonError::into_inner` and hand back state we cannot vouch
    /// for. Absent an explicit backend recovery contract, a poisoned lock is a
    /// backend failure the caller must see.
    fn lock(&self) -> Result<MutexGuard<'_, Box<dyn Store + Send>>, StoreError> {
        self.inner
            .lock()
            .map_err(|_| StoreError::backend(StorePoisoned))
    }

    /// Creates or overwrites the file at `path`. See [`Store::write`].
    ///
    /// # Errors
    /// Propagates any [`StoreError`] from the backend.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// store.write("a.txt", "hi")?;
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn write(&self, path: &str, contents: &str) -> Result<(), StoreError> {
        let path = StorePath::parse(path)?;
        self.lock()?.write(path.as_str(), contents)
    }

    /// Appends to the file at `path`, creating it if absent. See
    /// [`Store::append`].
    ///
    /// # Errors
    /// Propagates any [`StoreError`] from the backend.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// store.append("a.txt", "hi")?;
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn append(&self, path: &str, contents: &str) -> Result<(), StoreError> {
        let path = StorePath::parse(path)?;
        self.lock()?.append(path.as_str(), contents)
    }

    /// Reads the file at `path` as numbered lines. See
    /// [`Store::read_lines`].
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// store.write("a.txt", "hi")?;
    /// assert_eq!(store.read_lines("a.txt")?, "1| hi");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn read_lines(&self, path: &str) -> Result<String, StoreError> {
        let path = StorePath::parse(path)?;
        self.lock()?.read_lines(path.as_str())
    }

    /// Reads the file at `path` exactly as stored, with no line numbering.
    /// See [`Store::read`].
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// store.write("a.txt", "hi\n")?;
    /// assert_eq!(store.read("a.txt")?, "hi\n");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn read(&self, path: &str) -> Result<String, StoreError> {
        let path = StorePath::parse(path)?;
        self.lock()?.read(path.as_str())
    }

    /// Reads the file at `path` verbatim and wraps it in an untrusted guard
    /// envelope with a fresh nonce. Use this when stored content will be
    /// re-injected into a model-facing prompt.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// store.write("a.txt", "data")?;
    /// let wrapped = store.inject("a.txt")?;
    /// assert!(wrapped.contains("data"));
    /// assert!(wrapped.contains("untrusted_input_"));
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn inject(&self, path: &str) -> Result<String, StoreError> {
        let path = StorePath::parse(path)?;
        let contents = self.lock()?.read(path.as_str())?;
        Ok(crate::untrusted::wrap(&contents))
    }

    /// Replaces the unique occurrence of `old` with `new`. See
    /// [`Store::str_replace`].
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`], [`StoreError::AnchorNotFound`], or
    /// [`StoreError::AnchorAmbiguous`] per [`Store::str_replace`].
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// store.write("a.txt", "one two")?;
    /// store.str_replace("a.txt", "two", "three")?;
    /// assert_eq!(store.read_lines("a.txt")?, "1| one three");
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn str_replace(&self, path: &str, old: &str, new: &str) -> Result<(), StoreError> {
        let path = StorePath::parse(path)?;
        if old.is_empty() {
            // STORE-007: an empty anchor is a malformed edit request, not an
            // anchor that merely failed to match; refuse it with a dedicated
            // invalid-anchor condition before any backend search.
            return Err(StoreError::InvalidAnchor {
                path: path.as_str().to_owned(),
                reason: "anchor must not be empty",
            });
        }
        self.lock()?.str_replace(path.as_str(), old, new)
    }

    /// Removes the file at `path`. See [`Store::delete`].
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// store.write("a.txt", "hi")?;
    /// store.delete("a.txt")?;
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn delete(&self, path: &str) -> Result<(), StoreError> {
        let path = StorePath::parse(path)?;
        self.lock()?.delete(path.as_str())
    }

    /// Returns stored paths matching `pattern`, sorted. See [`Store::glob`].
    ///
    /// # Errors
    /// Propagates any [`StoreError`] from the backend.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// store.write("a.txt", "")?;
    /// store.write("b.md", "")?;
    /// assert_eq!(store.glob("*.txt")?, vec!["a.txt"]);
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn glob(&self, pattern: &str) -> Result<Vec<String>, StoreError> {
        if pattern.is_empty() {
            return Err(StoreError::InvalidPattern {
                pattern: pattern.to_owned(),
                reason: "pattern is empty".to_owned(),
            });
        }
        if pattern.len() > MAX_GLOB_PATTERN_BYTES {
            return Err(StoreError::InvalidPattern {
                pattern: pattern.to_owned(),
                reason: format!("pattern exceeds {MAX_GLOB_PATTERN_BYTES} bytes"),
            });
        }
        if pattern.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return Err(StoreError::InvalidPattern {
                pattern: pattern.to_owned(),
                reason: "pattern contains a control character".to_owned(),
            });
        }
        if let Err(reason) = validate_glob_grammar(pattern) {
            return Err(StoreError::InvalidPattern {
                pattern: pattern.to_owned(),
                reason: reason.to_owned(),
            });
        }
        // AUDIT-MUTEX-EXPENSIVE: snapshot every stored path under a brief lock
        // (a trivial `**` full enumeration), then release the lock and run the
        // arbitrary-pattern matcher on the owned snapshot. The O(tokens * path)
        // matching never executes while the shared backend mutex is held; only
        // the backend's own enumeration does.
        let snapshot = self.lock()?.glob("**")?;
        let tokens = compile_glob(pattern.as_bytes());
        Ok(snapshot
            .into_iter()
            .filter(|path| matches_tokens(&tokens, path.as_bytes()))
            .collect())
    }

    /// Returns whether a file exists at `path`. See [`Store::exists`].
    ///
    /// A confirmed absence is `Ok(false)`; a backend failure is `Err`.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidPath`] if `path` fails validation, or any
    /// [`StoreError`] the backend reports.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::store::StoreRef;
    ///
    /// let store = StoreRef::memory();
    /// assert!(!store.exists("a.txt")?);
    /// store.write("a.txt", "hi")?;
    /// assert!(store.exists("a.txt")?);
    /// # Ok::<(), promptforge_core::store::StoreError>(())
    /// ```
    pub fn exists(&self, path: &str) -> Result<bool, StoreError> {
        let path = StorePath::parse(path)?;
        self.lock()?.exists(path.as_str())
    }
}

#[cfg(test)]
mod tests;
