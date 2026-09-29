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
    /// Reads the file at `path` as stored.
    ///
    /// # Errors
    /// Returns an error when the policy denies the read, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let path = self.gate(Op::Read, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Read, &path);
        self.inner().read(&path)
    }

    /// Reads the file at `path` as UTF-8 text.
    ///
    /// # Errors
    /// Returns an error when the file's contents are not UTF-8.
    pub fn read_string(&self, path: &str) -> Result<String, VfsError> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes).map_err(|_| VfsError::NotUtf8 {
            path: path.to_owned(),
        })
    }

    /// Reads lines `start..=end` of the file at `path`, 1-based and
    /// inclusive, joined by `"\n"` with no trailing newline. An omitted
    /// `end` means the last line; a given `end` clamps down to it; a
    /// `start` past the last line reads as the empty string.
    ///
    /// # Errors
    /// Returns an error when `start` is below 1 or `end` is before
    /// `start`, when the file is missing, or when it is not UTF-8.
    pub fn read_range(
        &self,
        path: &str,
        start: usize,
        end: Option<usize>,
    ) -> Result<String, VfsError> {
        self.with_line_range(path, start, end, |lines, _| lines.join("\n"))
    }

    /// Reads lines `start..=end` as numbered lines, numbered absolutely
    /// from `start`, each right-aligned to the width of the largest
    /// emitted number and followed by `"| "`. Bounds behave as in
    /// [`Access::read_range`].
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

    /// Creates or overwrites the file at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the write, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn write(&self, path: &str, contents: &[u8]) -> Result<(), VfsError> {
        let path = self.gate(Op::Write, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Write, &path);
        self.inner().write(&path, contents)
    }

    /// Appends to the file at `path`, creating it if absent.
    ///
    /// # Errors
    /// Returns an error when the policy denies the append, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn append(&self, path: &str, contents: &[u8]) -> Result<(), VfsError> {
        let path = self.gate(Op::Append, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Append, &path);
        self.inner().append(&path, contents)
    }

    /// Replaces the unique occurrence of `old` with `new` in the file at
    /// `path`. An empty `old` is refused. Zero matches and multiple
    /// matches are both errors.
    ///
    /// # Errors
    /// Returns an error when `old` is empty, when the policy denies the
    /// write, when an access unordered with this one holds a conflicting
    /// claim on `path`, when the match count is not exactly one, or when
    /// the backend fails.
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
    /// A confirmed removal is `Ok(true)`, and a missing path is
    /// `Ok(false)`: deleting is idempotent.
    ///
    /// # Errors
    /// Returns an error when the policy denies the delete, when an access
    /// unordered with this one holds a conflicting claim - a recursive
    /// removal claims the whole subtree - or when the backend fails.
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

    /// A confirmed absence is `Ok(false)`; a backend failure is `Err`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the check, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn exists(&self, path: &str) -> Result<bool, VfsError> {
        let path = self.gate(Op::Exists, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Exists, &path);
        self.inner().exists(&path)
    }

    /// Returns the paths matching `pattern` that are files, or only
    /// directories when the pattern ends in `/`, sorted.
    ///
    /// The raw pattern is validated before canonicalization, so a
    /// backslash or a control character is refused rather than treated
    /// as a pattern byte, and a backslash is never turned into a
    /// separator. A pattern without a leading `/` joins onto the
    /// access's root, and its results come back relative to that root.
    ///
    /// # Errors
    /// Returns an error when the pattern is empty, over-long,
    /// control-bearing, backslash-bearing, or grammar-invalid, when a
    /// store view's strict path rules refuse it, when the policy denies
    /// the glob, when an access unordered with this one holds a
    /// conflicting claim (the pattern is the claim, not each match), or
    /// when the backend fails. Each malformed
    /// pattern reports the rule it broke as a [`PathReason`] in the
    /// [`VfsError::InvalidPath`].
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

    /// Lists the directory at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the list, when an access
    /// unordered with this one holds a conflicting claim on the
    /// directory's children, or when the backend fails.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, VfsError> {
        let path = self.gate(Op::List, path)?;
        self.admit(Claims::claim_list, &path)?;
        self.fire(Op::List, &path);
        self.inner().list(&path)
    }

    /// Returns metadata for `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the stat, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn stat(&self, path: &str) -> Result<Stat, VfsError> {
        let path = self.gate(Op::Stat, path)?;
        self.admit(Claims::claim_read, &path)?;
        self.fire(Op::Stat, &path);
        self.inner().stat(&path)
    }

    /// Creates the directory at `path`.
    ///
    /// # Errors
    /// Returns an error when the policy denies the mkdir, when an access
    /// unordered with this one holds a conflicting claim on `path`, or
    /// when the backend fails.
    pub fn mkdir(&self, path: &str, recursive: bool) -> Result<(), VfsError> {
        let path = self.gate(Op::Mkdir, path)?;
        self.admit(Claims::claim_write, &path)?;
        self.fire(Op::Mkdir, &path);
        self.inner().mkdir(&path, recursive)
    }

    /// Renames or moves, atomically where the backend allows. The source
    /// is claimed as the whole subtree it moves, the destination as a
    /// write.
    ///
    /// # Errors
    /// Returns an error when the policy denies the rename, when an access
    /// unordered with this one holds a conflicting claim on either path,
    /// or when the backend fails.
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

    /// Copies the file at `from` to `to`. The source is claimed as a
    /// read, the destination as a write.
    ///
    /// # Errors
    /// Returns an error when the policy denies the copy, when an access
    /// unordered with this one holds a conflicting claim on either path,
    /// or when the backend fails.
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
