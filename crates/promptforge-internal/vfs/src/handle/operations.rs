//! The capability's file operations: each gates its path, admits its
//! claims, and fires its event before it reaches the backend.

use super::Access;
use super::claims::Claims;
use super::store_view::{strip_root, validate_store_path};
use crate::error::{PathReason, VfsError};
use crate::glob::validate_glob_pattern;
use crate::stat::{Entry, Stat};
use crate::traits::Op;

impl Access {
    /// Reads the bytes of the file at `path`, exactly as stored.
    ///
    /// # Errors
    /// Returns an error when the policy denies the read, when another
    /// access holds a conflicting claim on `path` that is not ordered
    /// before this operation, or when the backend fails.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let path = self.gate(Op::Read, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Read, &path);
        self.inner().read(&path)
    }

    /// Reads the file at `path` as UTF-8 text.
    ///
    /// # Errors
    /// Returns an error under the same conditions as `Access::read`, or
    /// when the file's contents are not UTF-8.
    pub fn read_string(&self, path: &str) -> Result<String, VfsError> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes).map_err(|_| VfsError::NotUtf8 {
            path: path.to_owned(),
        })
    }

    /// Reads a range of lines from the file at `path` as text.
    ///
    /// Lines are numbered from 1, and the range runs from `start` to
    /// `end` with both ends included. The lines are joined by `"\n"` with
    /// no trailing newline. When `end` is `None`, the range runs to the
    /// last line. An `end` past the last line is lowered to the last
    /// line. A `start` past the last line returns the empty string.
    ///
    /// # Errors
    /// Returns an error when `start` is below 1 or `end` is before
    /// `start`. It also returns an error under the same conditions as
    /// `Access::read_string`, such as a missing file or contents that
    /// are not UTF-8.
    pub fn read_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, VfsError> {
        self.with_line_range(path, start, end, |lines, _| lines.join("\n"))
    }

    /// Reads a range of lines from the file at `path`, each prefixed with
    /// its line number.
    ///
    /// Each line keeps its number in the file, so the first line shown is
    /// numbered `start`. The numbers are right-aligned to the width of
    /// the largest number shown, and each is followed by `"| "`. The
    /// range bounds behave as in [`Access::read_range`].
    ///
    /// # Errors
    /// Returns an error under the same conditions as
    /// [`Access::read_range`].
    pub fn read_range_numbered(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, VfsError> {
        self.with_line_range(path, start, end, |lines, first| {
            let width = (first + lines.len() - 1).to_string().len();
            lines
                .iter()
                .enumerate()
                .map(|(index, line)| format!("{:>width$}| {}", first + index, line))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// Creates or overwrites the file at `path` with `contents`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the write, when another
    /// access holds a conflicting claim on `path` that is not ordered
    /// before this operation, or when the backend fails.
    pub fn write(&self, path: &str, contents: &[u8]) -> Result<(), VfsError> {
        let path = self.gate(Op::Write, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Write, &path);
        self.inner().write(&path, contents)
    }

    /// Appends `contents` to the file at `path`, creating the file if it
    /// is absent.
    ///
    /// # Errors
    /// Returns an error when the policy denies the append, when another
    /// access holds a conflicting claim on `path` that is not ordered
    /// before this operation, or when the backend fails.
    pub fn append(&self, path: &str, contents: &[u8]) -> Result<(), VfsError> {
        let path = self.gate(Op::Append, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Append, &path);
        self.inner().append(&path, contents)
    }

    /// Replaces the single occurrence of `old` with `new` in the file at
    /// `path`.
    ///
    /// An empty `old` is refused. It is an error when `old` occurs zero
    /// times or more than once.
    ///
    /// # Errors
    /// Returns an error when `old` is empty, when the policy denies the
    /// write, when another access holds a conflicting claim on `path`
    /// that is not ordered before this operation, when `old` does not
    /// occur exactly once, or when the backend fails.
    pub fn str_replace(&self, path: &str, old: &str, new: &str) -> Result<(), VfsError> {
        // The store view validates the path before the anchor, in the
        // store contract's order.
        if self.store_root.is_some() {
            validate_store_path(path)?;
        }
        if old.is_empty() {
            return Err(VfsError::Anchor {
                path: path.to_owned(),
                anchor: String::new(),
                count: 0,
            });
        }
        let path = self.gate(Op::Write, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Write, &path);
        self.inner().str_replace(&path, old, new)
    }

    /// Removes the file, link, or directory at `path`.
    ///
    /// Set `recursive` to also remove a directory that is not empty,
    /// along with everything under it. Returns `Ok(true)` when the
    /// removal succeeds and `Ok(false)` when nothing exists at `path`, so
    /// removing the same path twice is not an error.
    ///
    /// # Errors
    /// Returns an error when the policy denies the delete, when another
    /// access holds a conflicting claim that is not ordered before this
    /// operation, or when the backend fails. A recursive removal claims
    /// the whole subtree under `path`.
    pub fn remove(&self, path: &str, recursive: bool) -> Result<bool, VfsError> {
        let path = self.gate(Op::Delete, path)?;
        if recursive {
            self.admit(Claims::claim_subtree, &path)?;
        } else {
            self.admit(Claims::claim_write, &path)?;
        }
        self.fire(Op::Delete, &path);
        match self.inner().remove(&path, recursive) {
            Ok(()) => Ok(true),
            // The backend trait reports an absent path as NotFound; the
            // public capability confirms the absence instead.
            Err(VfsError::NotFound { .. }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Reports whether anything exists at `path`.
    ///
    /// Returns `Ok(false)` only when the path is confirmed absent. A
    /// backend failure is an error, never `Ok(false)`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the check, when another
    /// access holds a conflicting claim on `path` that is not ordered
    /// before this operation, or when the backend fails.
    pub fn exists(&self, path: &str) -> Result<bool, VfsError> {
        let path = self.gate(Op::Exists, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Exists, &path);
        self.inner().exists(&path)
    }

    /// Returns the sorted paths of the files, or of the directories, that
    /// match `pattern`.
    ///
    /// A pattern that ends in `/` matches only directories. Any other
    /// pattern matches only files. A pattern without a leading `/` is
    /// joined onto the access's root, and its results come back relative
    /// to that root.
    ///
    /// The raw pattern is validated before it is canonicalized. Because
    /// of this, a backslash or a control character is refused rather than
    /// treated as part of the pattern, and a backslash never becomes a
    /// path separator.
    ///
    /// # Errors
    /// Returns an error when the pattern is malformed: empty, too long,
    /// containing a control character or a backslash, or breaking the
    /// glob grammar. On a store view, an access from
    /// `VfsRef::acquire_store`, the pattern is also malformed when the
    /// store's strict path rules refuse it. Each malformed pattern
    /// reports the rule it broke as a [`PathReason`] in the
    /// [`VfsError::InvalidPath`]. It also returns an error when the
    /// policy denies the glob, when another access holds a conflicting
    /// claim that is not ordered before this operation, or when the
    /// backend fails. The claim covers the pattern itself, not each
    /// match.
    pub fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        if pattern.is_empty() {
            return Err(VfsError::InvalidPath {
                path: pattern.to_owned(),
                reason: PathReason::Empty,
            });
        }
        if let Err(reason) = validate_glob_pattern(pattern) {
            return Err(VfsError::InvalidPath {
                path: pattern.to_owned(),
                reason,
            });
        }
        // A trailing `/` asks for directories only; matching and the
        // claim key use the pattern without it.
        let dirs_only = pattern.ends_with('/');
        let stripped = pattern.trim_end_matches('/');
        let pattern = if stripped.is_empty() { "/" } else { stripped };
        self.glob_pattern(pattern, dirs_only)
    }

    /// The canonical-pattern half of [`Access::glob`]: gates, claims,
    /// fires, and matches one pattern whose trailing `/` was already
    /// split off into `dirs_only`. Crate-private so the mounted-handle
    /// forward can preserve the flag it received.
    pub(super) fn glob_pattern(
        &self,
        pattern: &str,
        dirs_only: bool,
    ) -> Result<Vec<String>, VfsError> {
        let relative = !pattern.starts_with('/');
        // The claim key is the canonicalized pattern; the backend
        // receives it canonical too.
        let claimed = self.gate(Op::Glob, pattern)?;
        self.admit(Claims::claim_glob, &claimed)?;
        self.fire(Op::Glob, &claimed);
        let mut matches = self.inner().glob_kind(claimed.as_str(), dirs_only)?;
        if relative {
            for matched in &mut matches {
                if let Some(rest) = strip_root(self.root.as_str(), matched) {
                    *matched = rest;
                }
            }
        }
        Ok(matches)
    }

    /// Lists the entries of the directory at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the list, when another
    /// access holds a conflicting claim on the directory's children that
    /// is not ordered before this operation, or when the backend fails.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, VfsError> {
        let path = self.gate(Op::List, path)?;
        self.admit(Claims::claim_list, &path)?;
        self.fire(Op::List, &path);
        self.inner().list(&path)
    }

    /// Returns metadata for `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the stat, when another
    /// access holds a conflicting claim on `path` that is not ordered
    /// before this operation, or when the backend fails.
    pub fn stat(&self, path: &str) -> Result<Stat, VfsError> {
        let path = self.gate(Op::Stat, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Stat, &path);
        self.inner().stat(&path)
    }

    /// Creates the directory at `path`.
    ///
    /// Set `recursive` to also create any missing parent directories.
    ///
    /// # Errors
    /// Returns an error when the policy denies the mkdir, when another
    /// access holds a conflicting claim on `path` that is not ordered
    /// before this operation, or when the backend fails.
    pub fn mkdir(&self, path: &str, recursive: bool) -> Result<(), VfsError> {
        let path = self.gate(Op::Mkdir, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Mkdir, &path);
        self.inner().mkdir(&path, recursive)
    }

    /// Renames or moves `from` to `to`.
    ///
    /// The move is atomic where the backend allows it. It claims the
    /// source as the whole subtree it moves, and the destination as a
    /// write.
    ///
    /// # Errors
    /// Returns an error when the policy denies the rename, when another
    /// access holds a conflicting claim on either path that is not
    /// ordered before this operation, or when the backend fails.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let from = self.gate(Op::Rename, from)?;
        let to = self.gate(Op::Rename, to)?;
        // Both paths gated, so the operation is admitted: the source
        // subtree and the destination are claimed, then one event per
        // canonical path.
        self.admit(
            |claims, scope, id, from| claims.claim_rename(scope, id, from, &to),
            &from,
        )?;
        self.fire(Op::Rename, &from);
        self.fire(Op::Rename, &to);
        self.inner().rename(&from, &to)
    }

    /// Copies the file at `from` to `to`.
    ///
    /// It claims the source as a read and the destination as a write.
    ///
    /// # Errors
    /// Returns an error when the policy denies the copy, when another
    /// access holds a conflicting claim on either path that is not
    /// ordered before this operation, or when the backend fails.
    pub fn copy(&self, from: &str, to: &str) -> Result<(), VfsError> {
        let from = self.gate(Op::Copy, from)?;
        let to = self.gate(Op::Copy, to)?;
        self.admit(
            |claims, scope, id, from| claims.claim_copy(scope, id, from, &to),
            &from,
        )?;
        self.fire(Op::Copy, &from);
        self.fire(Op::Copy, &to);
        self.inner().copy(&from, &to)
    }
}
