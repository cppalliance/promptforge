//! Run-scoped virtual files, shared by Lua and the model.
//!
//! A prompt run keeps its bulk state in virtual files addressed by logical
//! string paths. [`Store`] is a concrete facade over a prefix-scoped
//! [`Access`] capability from the shared VFS: the [`StoreExt`] extension
//! trait (re-exported in [`prelude`]) gives every `VfsRef` the
//! `vfs.store(&access)` call shape, binding the facade to the caller's
//! identity so its operations participate in the claims model - a second
//! live identity's conflicting write surfaces as [`StoreError::WriteRace`].
//!
//! The facade keeps the store's caller-facing contract: logical paths
//! validated before dispatch, verbatim reads, anchor-based edits
//! ([`Store::str_replace`]), 1-based inclusive line ranges with optional
//! absolute numbering, idempotent deletes, and the `*`/`**` glob grammar.
//! The `Store` trait, `MemStore`, `FileStore`, and the `WriteScope`
//! registry are gone: backends live in `shared-vfs`, the mount layout in
//! `promptforge-vfs`, and race detection in the claims model.
//!
//! This crate wires no execution; it defines the facade and its error
//! vocabulary only.

mod error;
mod path;

use std::fmt::Write as _;

use promptforge_vfs::STORE_MOUNT;
use shared_vfs::{FileType, VfsError, VfsRef};

pub use shared_vfs::Access;

pub use error::{PathReason, StoreError, StoreErrorKind};
use path::StorePath;

/// The largest glob pattern, in bytes, the facade will attempt to match.
///
/// The matcher is linear, but an unbounded pattern is still a cheap
/// denial-of-service lever, so an over-long pattern is refused outright.
pub(crate) const MAX_GLOB_PATTERN_BYTES: usize = 1024;

/// A concrete facade over one identity's prefix-scoped VFS capability.
///
/// A `Store` borrows the caller's [`Access`]: every operation is
/// attributed to the caller's identity and participates in its claims, so
/// a conflicting operation by a second live identity surfaces as
/// [`StoreError::WriteRace`]. Paths are logical (relative to the store
/// mount); the facade validates them, joins them onto the mount prefix,
/// and maps the VFS error vocabulary back onto [`StoreError`].
///
/// # Examples
/// ```
/// use promptforge_store::StoreExt;
///
/// let vfs = promptforge_vfs::empty();
/// let access = vfs.acquire();
/// let store = vfs.store(&access);
/// store.write("shared.txt", "state")?;
/// assert_eq!(store.read("shared.txt")?, "state");
/// # Ok::<(), promptforge_store::StoreError>(())
/// ```
#[derive(Debug, Clone)]
pub struct Store<'a> {
    access: &'a Access,
}

impl<'a> Store<'a> {
    /// Returns the facade over one identity's capability, scoped to the
    /// stock store mount.
    ///
    /// This is the constructor for holders that own the [`Access`] - the
    /// Lua VM's store closures build a facade per call over their shared
    /// `Arc<Access>`. Callers holding a `VfsRef` prefer the
    /// [`StoreExt::store`] shape.
    #[must_use]
    pub fn new(access: &'a Access) -> Store<'a> {
        Store { access }
    }
}

impl Store<'_> {
    /// Creates or overwrites the file at `path`.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidPath`] if `path` fails validation,
    /// [`StoreError::WriteRace`] if another live identity holds a claim on
    /// `path`, or [`StoreError::Backend`] if the backend fails.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.write("a.txt", "hi")?;
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    pub fn write(&self, path: &str, contents: &str) -> Result<(), StoreError> {
        let path = StorePath::parse(path)?;
        self.access
            .write(&full(path.as_str()), contents.as_bytes())
            .map_err(|err| map_vfs(err, path.as_str()))
    }

    /// Appends to the file at `path`, creating it if absent.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidPath`] if `path` fails validation,
    /// [`StoreError::WriteRace`] if another live identity holds a claim on
    /// `path`, or [`StoreError::Backend`] if the backend fails.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.append("a.txt", "hi")?;
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    pub fn append(&self, path: &str, contents: &str) -> Result<(), StoreError> {
        let path = StorePath::parse(path)?;
        self.access
            .append(&full(path.as_str()), contents.as_bytes())
            .map_err(|err| map_vfs(err, path.as_str()))
    }

    /// Reads the file at `path` exactly as stored, with no line numbering.
    ///
    /// This is the accessor for verbatim handoff, clean dumps, and trusted
    /// re-injection. Numbered output for navigation is derived from a read
    /// at this layer.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.write("a.txt", "hi\n")?;
    /// assert_eq!(store.read("a.txt")?, "hi\n");
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    pub fn read(&self, path: &str) -> Result<String, StoreError> {
        let path = StorePath::parse(path)?;
        self.access
            .read_string(&full(path.as_str()))
            .map_err(|err| map_vfs(err, path.as_str()))
    }

    /// Reads lines `start..=end` of the file at `path`, 1-based and
    /// inclusive, joined with `"\n"` and no trailing newline.
    ///
    /// Bounds are evaluated in a fixed order: a `start` below 1 is an error;
    /// a `start` past the last line reads as the empty string; an omitted
    /// `end` means the last line, and a given `end` clamps down to it; an
    /// `end` before `start` at that point is an error.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`, or
    /// [`StoreError::InvalidRange`] if `start` is less than 1 or `end` is
    /// before `start`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.write("a.txt", "one\ntwo\nthree\n")?;
    /// assert_eq!(store.read_range("a.txt", 2, None)?, "two\nthree");
    /// assert_eq!(store.read_range("a.txt", 2, Some(99))?, "two\nthree");
    /// assert_eq!(store.read_range("a.txt", 99, None)?, "");
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    pub fn read_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, StoreError> {
        self.with_read_range(path, start, end, |lines, _| lines.join("\n"))
    }

    /// Reads lines `start..=end` of the file at `path` as numbered lines,
    /// 1-based and inclusive, numbered absolutely from `start`.
    ///
    /// Each line is prefixed with its number, right-aligned to the width of
    /// the largest emitted number, followed by `"| "`; lines are joined with
    /// `"\n"` and there is no trailing newline. With `start` of 1 and no
    /// `end` the whole file is numbered from 1. Bounds are evaluated exactly
    /// as in [`Store::read_range`]: a `start` below 1 is an error; a
    /// `start` past the last line reads as the empty string; an omitted
    /// `end` means the last line, and a given `end` clamps down to it; an
    /// `end` before `start` at that point is an error.
    ///
    /// # Errors
    /// Returns [`StoreError::NotFound`] if no file exists at `path`, or
    /// [`StoreError::InvalidRange`] if `start` is less than 1 or `end` is
    /// before `start`.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.write("a.txt", "one\ntwo\nthree\n")?;
    /// assert_eq!(
    ///     store.read_range_numbered("a.txt", 1, None)?,
    ///     "1| one\n2| two\n3| three"
    /// );
    /// assert_eq!(store.read_range_numbered("a.txt", 2, Some(3))?, "2| two\n3| three");
    /// assert_eq!(store.read_range_numbered("a.txt", 99, None)?, "");
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    pub fn read_range_numbered(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, StoreError> {
        self.with_read_range(path, start, end, number_lines_from)
    }

    /// Reads and resolves one line range while its owned contents remain live.
    fn with_read_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
        render: impl FnOnce(&[&str], usize) -> String,
    ) -> Result<String, StoreError> {
        let path = StorePath::parse(path)?;
        let contents = self.read(path.as_str())?;
        let lines: Vec<&str> = contents.lines().collect();
        let Some((start, end)) = resolve_line_range(path.as_str(), lines.len(), start, end)? else {
            return Ok(String::new());
        };
        Ok(render(&lines[start - 1..end], start))
    }

    /// Replaces the unique occurrence of `old` with `new`.
    ///
    /// The edit is anchor-based: `old` must occur exactly once. Zero matches
    /// and more-than-one match are both refused, so an edit never lands on
    /// an arbitrary match.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidAnchor`] when `old` is empty,
    /// [`StoreError::NotFound`] if no file exists at `path`,
    /// [`StoreError::AnchorNotFound`] if `old` does not occur, or
    /// [`StoreError::AnchorAmbiguous`] if `old` occurs more than once.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.write("a.txt", "one two")?;
    /// store.str_replace("a.txt", "two", "three")?;
    /// assert_eq!(store.read("a.txt")?, "one three");
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    pub fn str_replace(&self, path: &str, old: &str, new: &str) -> Result<(), StoreError> {
        let path = StorePath::parse(path)?;
        if old.is_empty() {
            // STORE-007: an empty anchor is a malformed edit request, not an
            // anchor that merely failed to match; refuse it with a dedicated
            // invalid-anchor condition before any search.
            return Err(StoreError::InvalidAnchor {
                path: path.as_str().to_owned(),
                reason: "anchor must not be empty",
            });
        }
        let contents = self.read(path.as_str())?;
        let count = contents.matches(old).count();
        match count {
            0 => Err(StoreError::AnchorNotFound {
                path: path.as_str().to_owned(),
                anchor: old.to_owned(),
            }),
            1 => {
                let replaced = contents.replacen(old, new, 1);
                self.write(path.as_str(), &replaced)
            }
            count => Err(StoreError::AnchorAmbiguous {
                path: path.as_str().to_owned(),
                anchor: old.to_owned(),
                count,
            }),
        }
    }

    /// Removes the file at `path`.
    ///
    /// Delete is idempotent: a missing file is not an error.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidPath`] if `path` fails validation, or
    /// [`StoreError::Backend`] if the backend fails.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.write("a.txt", "hi")?;
    /// store.delete("a.txt")?;
    /// store.delete("a.txt")?; // already gone; still Ok
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    pub fn delete(&self, path: &str) -> Result<(), StoreError> {
        let path = StorePath::parse(path)?;
        match self.access.remove(&full(path.as_str()), false) {
            // Idempotent: an absent path is already in the post-delete state.
            Ok(()) | Err(VfsError::NotFound(_)) => Ok(()),
            Err(other) => Err(map_vfs(other, path.as_str())),
        }
    }

    /// Returns stored paths matching `pattern`, sorted.
    ///
    /// Two wildcards are supported: `*` matches any run of characters within
    /// a single path segment (it never crosses `/`), and `**` matches any
    /// run of characters including `/`. All other characters match
    /// literally. Only files are listed: the store vocabulary has no
    /// directories.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidPattern`] if `pattern` is empty,
    /// over-long, control-bearing, or grammar-invalid,
    /// [`StoreError::WriteRace`] if another live identity holds a writer
    /// claim on a matched path, or [`StoreError::Backend`] if the backend
    /// fails.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.write("a.txt", "")?;
    /// store.write("b.md", "")?;
    /// assert_eq!(store.glob("*.txt")?, vec!["a.txt"]);
    /// # Ok::<(), promptforge_store::StoreError>(())
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
        // The grammar has no escape syntax, and the router canonicalizes
        // patterns (separators included) before the backend can reject
        // them, so the backslash refusal must happen here.
        if pattern.contains('\\') {
            return Err(StoreError::InvalidPattern {
                pattern: pattern.to_owned(),
                reason: "pattern does not support backslash escapes".to_owned(),
            });
        }
        // One glob implementation lives in shared-vfs; the facade scopes
        // the pattern to the mount and maps a grammar rejection back onto
        // the store vocabulary.
        let scoped = format!("{STORE_MOUNT}/{pattern}");
        let matches = self.access.glob(&scoped).map_err(|err| match err {
            VfsError::InvalidPath(reason) => StoreError::InvalidPattern {
                pattern: pattern.to_owned(),
                reason,
            },
            other => map_vfs(other, pattern),
        })?;
        let prefix = format!("{STORE_MOUNT}/");
        let mut paths = Vec::new();
        for matched in matches {
            // The VFS glob lists directories as well as files; the store
            // vocabulary lists files only.
            let logical = matched.strip_prefix(&prefix).unwrap_or(&matched);
            let stat = self
                .access
                .stat(&matched)
                .map_err(|err| map_vfs(err, logical))?;
            if stat.file_type != FileType::File {
                continue;
            }
            paths.push(logical.to_owned());
        }
        Ok(paths)
    }

    /// Returns whether a file exists at `path`.
    ///
    /// A confirmed absence is `Ok(false)`; a backend failure is `Err`.
    ///
    /// # Errors
    /// Returns [`StoreError::InvalidPath`] if `path` fails validation, or
    /// [`StoreError::Backend`] if the backend fails.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// assert!(!store.exists("a.txt")?);
    /// store.write("a.txt", "hi")?;
    /// assert!(store.exists("a.txt")?);
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    pub fn exists(&self, path: &str) -> Result<bool, StoreError> {
        let path = StorePath::parse(path)?;
        self.access
            .exists(&full(path.as_str()))
            .map_err(|err| map_vfs(err, path.as_str()))
    }
}

/// The extension trait behind the `vfs.store(&access)` call shape.
///
/// The [`Store`] facade type lives in this crate, above `promptforge-vfs`
/// and `shared-vfs` in the dependency stack, so the method cannot be
/// inherent on `VfsRef`; a prelude-exported extension trait preserves the
/// declared call shape without inverting the stack.
pub trait StoreExt {
    /// Returns the store facade scoped to the stock store mount, bound to
    /// `access`'s identity.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreExt;
    ///
    /// let vfs = promptforge_vfs::empty();
    /// let access = vfs.acquire();
    /// let store = vfs.store(&access);
    /// store.write("seeded.txt", "input")?;
    /// # Ok::<(), promptforge_store::StoreError>(())
    /// ```
    fn store<'a>(&self, access: &'a Access) -> Store<'a>;
}

impl StoreExt for VfsRef {
    fn store<'a>(&self, access: &'a Access) -> Store<'a> {
        Store { access }
    }
}

/// The integrator prelude: the facade and its extension trait.
pub mod prelude {
    pub use crate::{Store, StoreExt};
}

/// Joins a validated logical path onto the store mount prefix.
fn full(path: &str) -> String {
    format!("{STORE_MOUNT}/{path}")
}

/// Maps the VFS error vocabulary onto the store's, keeping the logical
/// path the caller supplied. A claim conflict is the write-write race the
/// claims model detects; everything without a store-vocabulary home is an
/// opaque backend failure.
fn map_vfs(err: VfsError, path: &str) -> StoreError {
    match err {
        VfsError::NotFound(_) => StoreError::NotFound {
            path: path.to_owned(),
        },
        VfsError::Conflict(_) => StoreError::WriteRace {
            path: path.to_owned(),
        },
        other => StoreError::backend(other),
    }
}

/// Resolves 1-based inclusive bounds against `line_count` into the effective
/// `(start, end)`, or `None` when the range falls entirely past the last
/// line. Evaluation order is fixed: a `start` below 1 is an error; a `start`
/// past the last line reads as empty; an omitted `end` means the last line,
/// and a given `end` clamps down to it; an `end` before `start` at that
/// point is an error.
fn resolve_line_range(
    path: &str,
    line_count: usize,
    start: usize,
    end: Option<usize>,
) -> Result<Option<(usize, usize)>, StoreError> {
    if start == 0 {
        return Err(StoreError::InvalidRange {
            path: path.to_owned(),
            reason: "start must be at least 1",
        });
    }
    if start > line_count {
        return Ok(None);
    }
    let end = end.unwrap_or(line_count).min(line_count);
    if end < start {
        return Err(StoreError::InvalidRange {
            path: path.to_owned(),
            reason: "end must not be before start",
        });
    }
    Ok(Some((start, end)))
}

/// Renders `lines` numbered absolutely from `start`, each number
/// right-aligned to the width of the largest emitted number, followed by
/// `"| "`; lines are joined with `"\n"` and there is no trailing newline.
fn number_lines_from(lines: &[&str], start: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let last = start + lines.len() - 1;
    let width = last.to_string().len();
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let number = start + index;
        // Writing to a String is infallible; the result carries no information.
        let _ = write!(out, "{number:>width$}| {line}");
    }
    out
}

#[cfg(test)]
mod tests;
