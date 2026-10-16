//! The backend traits: [`Vfs`], one backend behind the virtual namespace,
//! and [`VfsAccess`], one identity's session with it, plus the enums the
//! session's enum-taking operations take.

use super::identity::{AcquireContext, ExecId};
use crate::error::VfsError;
use crate::path::{VfsPath, VfsPathBuf, canonicalize_absolute};
use crate::stat::{Entry, FileType, Stat};

/// How far a removal or a directory creation reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Depth {
    /// Only the named path: a removal refuses a directory that still has
    /// entries, and a creation refuses a missing parent directory.
    One,
    /// The named path and every path it needs: a removal takes the whole
    /// subtree, and a creation makes every missing parent directory.
    Recursive,
}

impl Depth {
    /// The `recursive` flag of the `bool` forms.
    pub(crate) fn recursive(self) -> bool {
        match self {
            Depth::One => false,
            Depth::Recursive => true,
        }
    }
}

/// Which matches of a glob pattern to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GlobKind {
    /// Only the matches that are files.
    FilesOnly,
    /// Only the matches that are directories.
    DirsOnly,
}

/// A storage backend behind the virtual filesystem.
///
/// Operations are synchronous because the Lua VM and the single thread
/// that drives a run are both synchronous. They work on raw bytes. A
/// backend must be `Send`, and `Sync` is optional, because `VfsRef`
/// serializes access to it. Storage is reachable only through an access
/// object that `acquire` binds to one identity.
pub trait Vfs: Send {
    /// Acquires an access object bound to the identity in `cx`.
    ///
    /// Every operation on the returned object is attributed to
    /// [`AcquireContext::id`]. A backend may use the identity to track
    /// who touches what, or ignore it. A backend that wraps another
    /// [`Vfs`] must pass it `cx` exactly as received.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend fails to open a session.
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError>;

    /// Ends the backend session for `id`.
    ///
    /// The access object calls this when it is dropped, so cancellation,
    /// panics, and early returns cannot skip it. When backends are
    /// mounted at several paths, each mount the identity touched gets one
    /// release. A release ends only the backend session. The identity's
    /// happens-before claims end with its scope.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend fails to release the identity.
    fn release(&mut self, id: ExecId) -> Result<(), VfsError>;

    /// Whether this backend rejects all mutations.
    fn read_only(&self) -> bool {
        false
    }
}

/// One identity's session with a backend, through which every
/// filesystem operation runs.
///
/// Every operation on a session is attributed to the identity it was
/// acquired for. Storage is reachable only through a session. Paths
/// arrive validated, canonicalized, and interned, so a backend uses
/// them as given.
pub trait VfsAccess: Send {
    /// Reads the file at `path` as stored.
    ///
    /// # Errors
    ///
    /// Returns [`VfsError::NotFound`] when the file is absent, or a
    /// backend error when the read fails.
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError>;

    /// Reads up to `len` bytes starting at byte `offset`.
    ///
    /// The default reads the whole file and slices it. Backends that can
    /// seek, such as `RealBackend`, override it to read only the
    /// requested bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying read fails or the range
    /// exceeds the addressable size.
    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        let data = self.read(path)?;
        let Ok(start) = usize::try_from(offset) else {
            return Err(VfsError::Backend {
                message: format!("read_range offset {offset} exceeds the addressable size"),
            });
        };
        let Ok(length) = usize::try_from(len) else {
            return Err(VfsError::Backend {
                message: format!("read_range length {len} exceeds the addressable size"),
            });
        };
        if start >= data.len() {
            return Ok(Vec::new());
        }
        let end = data.len().min(start.saturating_add(length));
        Ok(data[start..end].to_vec())
    }

    /// Creates or overwrites the file at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend fails to write the contents.
    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError>;

    /// Appends to the file at `path`, creating it if absent.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend fails to append the contents.
    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError>;

    /// Removes the file, link, or directory at `path`.
    ///
    /// Removing a directory that has entries requires `recursive`. On a
    /// symbolic link, it removes the link and never the target.
    ///
    /// # Errors
    ///
    /// Returns [`VfsError::NotFound`] when the path is absent, or an
    /// error when the removal fails.
    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError>;

    /// Removes the file, link, or directory at `path`, reaching as far as
    /// `depth`.
    ///
    /// The default calls [`VfsAccess::remove`] with `recursive` set for
    /// [`Depth::Recursive`].
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as
    /// [`VfsAccess::remove`].
    fn remove_with(&mut self, path: &VfsPath, depth: Depth) -> Result<(), VfsError> {
        self.remove(path, depth.recursive())
    }

    /// Reports whether `path` exists.
    ///
    /// A confirmed absence returns `Ok(false)`, and a backend failure
    /// returns `Err`.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend fails to determine existence.
    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError>;

    /// Returns stored paths matching `pattern`, sorted.
    ///
    /// # Errors
    ///
    /// Returns an error when the pattern is invalid or the backend fails.
    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError>;

    /// Returns the stored paths matching `pattern` that are files, or
    /// directories when `dirs_only` is set, sorted.
    ///
    /// The default calls [`VfsAccess::glob`] and keeps each match whose
    /// [`VfsAccess::stat`] shows the wanted type. Backends that index
    /// their own trees override it to read each match's type from that
    /// index.
    ///
    /// # Errors
    ///
    /// Returns an error when the pattern is invalid, when a matched path
    /// fails to canonicalize or stat, or when the backend fails.
    fn glob_kind(&self, pattern: &str, dirs_only: bool) -> Result<Vec<String>, VfsError> {
        let mut kept = Vec::new();
        for matched in self.glob(pattern)? {
            let stat = self.stat(&canonicalize_absolute(&matched)?)?;
            let keep = if dirs_only {
                stat.file_type == FileType::Directory
            } else {
                stat.file_type == FileType::File
            };
            if keep {
                kept.push(matched);
            }
        }
        Ok(kept)
    }

    /// Returns the stored paths matching `pattern` that `kind` keeps,
    /// sorted.
    ///
    /// The default calls [`VfsAccess::glob_kind`], with `dirs_only` clear
    /// for [`GlobKind::FilesOnly`] and set for [`GlobKind::DirsOnly`].
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as
    /// [`VfsAccess::glob_kind`].
    fn glob_with(&self, pattern: &str, kind: GlobKind) -> Result<Vec<String>, VfsError> {
        match kind {
            GlobKind::FilesOnly => self.glob_kind(pattern, false),
            GlobKind::DirsOnly => self.glob_kind(pattern, true),
        }
    }

    /// Lists the directory at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is not a directory or the
    /// backend fails.
    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError>;

    /// Returns metadata for `path`.
    ///
    /// # Errors
    ///
    /// Returns [`VfsError::NotFound`] when the path is absent, or a
    /// backend error when the stat fails.
    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError>;

    /// Creates the directory at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error when creating the directory fails.
    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError>;

    /// Creates the directory at `path`, making missing parents when
    /// `depth` is [`Depth::Recursive`].
    ///
    /// The default calls [`VfsAccess::mkdir`].
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as
    /// [`VfsAccess::mkdir`].
    fn mkdir_with(&mut self, path: &VfsPath, depth: Depth) -> Result<(), VfsError> {
        self.mkdir(path, depth.recursive())
    }

    /// Renames or moves `from` to `to`, atomically where the backend
    /// allows.
    ///
    /// # Errors
    ///
    /// Returns an error when the rename fails; source and destination
    /// stay as they were.
    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError>;

    /// Copies the file at `from` to `to`.
    ///
    /// # Errors
    ///
    /// Returns an error when the copy fails; source and destination
    /// stay as they were.
    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError>;

    /// Replaces the single occurrence of `old` with `new` in the file at
    /// `path`.
    ///
    /// The default reads the file, counts the matches, replaces the one
    /// match, and writes the file back. A backend can override it to do
    /// the replacement itself.
    ///
    /// # Errors
    ///
    /// Returns an error when `old` is empty, when the file holds invalid
    /// UTF-8, when `old` matches zero times or more than once, or when
    /// the read or write fails.
    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str) -> Result<(), VfsError> {
        if old.is_empty() {
            return Err(VfsError::Anchor {
                path: path.to_string(),
                anchor: String::new(),
                count: 0,
            });
        }
        let bytes = self.read(path)?;
        let text = String::from_utf8(bytes).map_err(|err| VfsError::NotUtf8 {
            path: path.to_string(),
            valid_up_to: err.utf8_error().valid_up_to(),
        })?;
        let count = text.matches(old).count();
        match count {
            0 => Err(VfsError::Anchor {
                path: path.to_string(),
                anchor: old.to_owned(),
                count: 0,
            }),
            1 => {
                let replaced = text.replacen(old, new, 1);
                self.write(path, replaced.as_bytes())
            }
            count => Err(VfsError::Anchor {
                path: path.to_string(),
                anchor: old.to_owned(),
                count,
            }),
        }
    }

    /// Creates a symbolic link at `link` naming `target`.
    ///
    /// This is an optional POSIX operation.
    ///
    /// # Errors
    ///
    /// The default returns [`VfsError::Unsupported`].
    fn symlink(&mut self, target: &VfsPath, link: &VfsPath) -> Result<(), VfsError> {
        let _ = target;
        Err(VfsError::Unsupported {
            path: link.to_string(),
            detail: format!("symlink is not supported by this backend: {link}"),
        })
    }

    /// Reads the target of the symbolic link at `path`.
    ///
    /// This is an optional POSIX operation.
    ///
    /// # Errors
    ///
    /// The default returns [`VfsError::Unsupported`].
    fn read_link(&self, path: &VfsPath) -> Result<VfsPathBuf, VfsError> {
        Err(VfsError::Unsupported {
            path: path.to_string(),
            detail: format!("read_link is not supported by this backend: {path}"),
        })
    }

    /// Changes the mode bits of `path`.
    ///
    /// This is an optional POSIX operation.
    ///
    /// # Errors
    ///
    /// The default returns [`VfsError::Unsupported`].
    fn chmod(&mut self, path: &VfsPath, mode: u32) -> Result<(), VfsError> {
        let _ = mode;
        Err(VfsError::Unsupported {
            path: path.to_string(),
            detail: format!("chmod is not supported by this backend: {path}"),
        })
    }

    /// Ends the session's hold on what it acquired, reporting the
    /// failures that dropping the session would swallow.
    ///
    /// The access object calls this before it drops the session, so a
    /// session that releases inner sessions, such as one spanning several
    /// mounts, releases them here and holds nothing to release once
    /// dropped. The default holds nothing and returns `Ok`.
    ///
    /// # Errors
    ///
    /// Returns the first release that failed.
    fn close(&mut self) -> Result<(), VfsError> {
        Ok(())
    }
}
