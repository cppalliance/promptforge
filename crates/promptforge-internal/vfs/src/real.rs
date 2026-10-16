//! The real-filesystem backend.
//!
//! [`RealBackend`] serves real directories behind the virtual
//! namespace over direct `std::fs` calls. Two constructors:
//! [`RealBackend::identity`] (the virtual path IS the real path) and
//! [`RealBackend::rooted`] (chroot-style, with lexical plus
//! canonicalize containment). Writes, copies, and renames are
//! failure-atomic: a sibling temp file plus rename, so a failed
//! operation leaves source, destination, and accounting unchanged.
//!
//! Paths resolve two ways. Operations on a path itself (`remove`,
//! `exists`, `stat`, `mkdir`, `rename`) contain the parent and act on a
//! final-component link as a link, never its target. Operations on
//! contents (`read`, `read_range`, `write`, `append`, `list`, `glob`,
//! `copy`) follow links under the containment check, which refuses a
//! link that resolves outside the root and a path that passes through
//! a dangling link.

mod files;
mod resolve;
#[cfg(test)]
mod tests;

#[cfg(unix)]
#[path = "real/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "real/windows.rs"]
mod platform;
#[cfg(not(any(unix, windows)))]
#[path = "real/fallback.rs"]
mod platform;

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::{PathReason, VfsError};
use crate::glob::{compile_glob, matches_tokens, validate_glob_pattern};
use crate::path::{VfsPath, canonicalize_absolute};
use crate::stat::{Entry, Stat};
use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

use files::{atomic_write, create_parent, stat_of, utf8_name, walk, walk_root};
use resolve::{RealRoot, contain, contain_no_follow, join_virtual};

/// Maps an I/O failure to the error kind the trait surface promises.
/// Each `path` field holds the canonical path alone, so a caller reading
/// the field per its documented contract gets a path, never the OS
/// error's text. A failure outside the named kinds keeps its kind, its
/// OS error code, and the OS text beside the path in `Io`.
fn map_io(path: &str, err: &std::io::Error) -> VfsError {
    let path = path.to_owned();
    match err.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound { path },
        std::io::ErrorKind::AlreadyExists => VfsError::AlreadyExists { path },
        std::io::ErrorKind::IsADirectory => VfsError::IsADirectory { path },
        std::io::ErrorKind::NotADirectory => VfsError::NotADirectory { path },
        std::io::ErrorKind::DirectoryNotEmpty => VfsError::DirectoryNotEmpty { path },
        kind => VfsError::Io {
            path,
            kind,
            raw_os_error: err.raw_os_error(),
            message: err.to_string(),
        },
    }
}

/// Whether a [`RealBackend`] accepts mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Writability {
    /// Reads and mutations are both served.
    ReadWrite,
    /// Reads are served, and every mutation is refused with
    /// [`VfsError::ReadOnly`].
    ReadOnly,
}

/// A filesystem backend that maps virtual paths onto real files and
/// directories.
///
/// Each operation calls `std::fs` directly. A backend built with
/// [`RealBackend::rooted`] keeps every path inside its root directory.
/// It checks containment both lexically and after canonicalizing the
/// path. Writes, copies, and renames are failure-atomic.
///
/// On Windows and macOS, whose file systems treat paths that differ only
/// in case as one file, the claims check of a handle that mounts a
/// rooted backend keys each path below the mount by its lowercase
/// spelling, so two spellings of one file conflict as one path does.
///
/// The backend accepts an `ExecId` for attribution and ignores it,
/// because every identity sees the same real filesystem.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RealBackend {
    root: RealRoot,
    read_only: bool,
}

impl RealBackend {
    /// Creates a backend whose virtual paths are the real paths.
    ///
    /// Virtual `/a/b` is real `/a/b`. On Windows, virtual `/C:/a/b` is
    /// real `C:\a\b`. The backend applies no containment check.
    #[must_use]
    pub fn identity() -> RealBackend {
        RealBackend {
            root: RealRoot::Identity,
            read_only: false,
        }
    }

    /// Creates a backend rooted at the real directory `dir`, like a
    /// `chroot`.
    ///
    /// The virtual root `/` maps to `dir`. The constructor canonicalizes
    /// `dir` and checks that it is a directory. Every path the backend
    /// resolves must stay inside the canonical root.
    ///
    /// # Errors
    ///
    /// Returns [`VfsError::NotUtf8`] when `dir` is not UTF-8,
    /// [`VfsError::NotFound`] when `dir` is absent, or
    /// [`VfsError::NotADirectory`] when it is not a directory.
    pub fn rooted(dir: impl AsRef<Path>) -> Result<RealBackend, VfsError> {
        let dir = dir.as_ref();
        let bytes = dir.as_os_str().as_encoded_bytes();
        let display = std::str::from_utf8(bytes).map_err(|err| VfsError::NotUtf8 {
            // The bytes before `valid_up_to` are UTF-8 by the error's own
            // account.
            path: std::str::from_utf8(&bytes[..err.valid_up_to()])
                .unwrap_or_default()
                .to_owned(),
            valid_up_to: err.valid_up_to(),
        })?;
        let canonical = fs::canonicalize(dir).map_err(|err| map_io(display, &err))?;
        if !canonical.is_dir() {
            return Err(VfsError::NotADirectory {
                path: display.to_owned(),
            });
        }
        Ok(RealBackend {
            root: RealRoot::Rooted(canonical),
            read_only: false,
        })
    }

    /// Sets whether the backend rejects all mutations.
    ///
    /// The flag is a property of the mount, separate from any policy.
    #[must_use]
    pub fn with_read_only(mut self, read_only: bool) -> RealBackend {
        self.read_only = read_only;
        self
    }

    /// Sets whether the backend accepts mutations, as
    /// [`RealBackend::with_read_only`] does.
    #[must_use]
    pub fn with_writability(self, writability: Writability) -> RealBackend {
        self.with_read_only(match writability {
            Writability::ReadWrite => false,
            Writability::ReadOnly => true,
        })
    }

    /// Whether the claims over this backend fold case: a rooted backend
    /// on a platform whose file systems fold it.
    pub(crate) fn folds_case(&self) -> bool {
        match self.root {
            RealRoot::Identity => false,
            RealRoot::Rooted(_) => platform::FOLDS_CASE,
        }
    }
}

impl Vfs for RealBackend {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        // Attribution is accepted as a no-op: the real filesystem holds
        // no per-identity state, and the claims model above the backend
        // enforces conflicts.
        let _ = cx;
        Ok(Box::new(RealAccess {
            root: self.root.clone(),
            read_only: self.read_only,
        }))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        let _ = id;
        Ok(())
    }

    fn read_only(&self) -> bool {
        self.read_only
    }
}

/// One identity's session with a [`RealBackend`]. The identity is
/// dropped on the floor: attribution is a no-op.
struct RealAccess {
    root: RealRoot,
    read_only: bool,
}

impl RealAccess {
    /// Resolves a canonical virtual path to its real path, applying
    /// containment in rooted mode.
    fn resolve(&self, path: &VfsPath) -> Result<PathBuf, VfsError> {
        match &self.root {
            RealRoot::Identity => Ok(platform::identity_to_real(path.as_str())),
            RealRoot::Rooted(root) => {
                let candidate = join_virtual(root, path.as_str());
                contain(root, &candidate, path)
            }
        }
    }

    /// Resolves a canonical virtual path to its real path without
    /// following a final-component link, applying containment to the
    /// parent in rooted mode.
    fn resolve_no_follow(&self, path: &VfsPath) -> Result<PathBuf, VfsError> {
        match &self.root {
            RealRoot::Identity => Ok(platform::identity_to_real(path.as_str())),
            RealRoot::Rooted(root) => {
                let candidate = join_virtual(root, path.as_str());
                contain_no_follow(root, &candidate, path)
            }
        }
    }

    /// Rejects mutations on a read-only backend before anything is
    /// touched: a denied operation never partially applies.
    fn check_writable(&self, path: &VfsPath) -> Result<(), VfsError> {
        if self.read_only {
            return Err(VfsError::ReadOnly {
                path: path.to_string(),
            });
        }
        Ok(())
    }
}

impl VfsAccess for RealAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let real = self.resolve(path)?;
        if real.is_dir() {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        fs::read(&real).map_err(|err| map_io(path.as_str(), &err))
    }

    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        // Seek, never materialize: a real file can position directly.
        let real = self.resolve(path)?;
        if real.is_dir() {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        let mut file = File::open(&real).map_err(|err| map_io(path.as_str(), &err))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|err| map_io(path.as_str(), &err))?;
        let mut buffer = Vec::new();
        file.take(len)
            .read_to_end(&mut buffer)
            .map_err(|err| map_io(path.as_str(), &err))?;
        Ok(buffer)
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let real = self.resolve(path)?;
        if real.is_dir() {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        create_parent(&real, path)?;
        atomic_write(&real, path, contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let real = self.resolve(path)?;
        if real.is_dir() {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        create_parent(&real, path)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&real)
            .map_err(|err| map_io(path.as_str(), &err))?;
        file.write_all(contents)
            .map_err(|err| map_io(path.as_str(), &err))
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.check_writable(path)?;
        if path.as_str() == "/" {
            return Err(VfsError::RootProtected {
                path: path.to_string(),
            });
        }
        let real = self.resolve_no_follow(path)?;
        let metadata = fs::symlink_metadata(&real).map_err(|err| map_io(path.as_str(), &err))?;
        // symlink_metadata does not follow links: a symlink is removed
        // as a link, never its target.
        if metadata.is_dir() {
            if recursive {
                fs::remove_dir_all(&real)
            } else {
                fs::remove_dir(&real)
            }
        } else if platform::is_dir_link(&metadata) {
            fs::remove_dir(&real)
        } else {
            fs::remove_file(&real)
        }
        .map_err(|err| map_io(path.as_str(), &err))
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        let real = self.resolve_no_follow(path)?;
        // symlink_metadata counts a dangling link as existing. Only a
        // confirmed absence is Ok(false); every other failure (a
        // denied permission, a genuine I/O error) surfaces as Err, as
        // the trait contract requires.
        match fs::symlink_metadata(&real) {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(map_io(path.as_str(), &err)),
        }
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        if let Err(reason) = validate_glob_pattern(pattern) {
            return Err(VfsError::InvalidPath {
                path: pattern.to_owned(),
                reason,
            });
        }
        let tokens = compile_glob(pattern.as_bytes());
        // The walk spells each match from the virtual walk root, so a
        // link the root resolves through keeps the pattern's spelling.
        let walk_from = canonicalize_absolute(walk_root(pattern))?;
        let root = self.resolve(&walk_from)?;
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        walk(&root, walk_from.as_str(), &mut found)?;
        let mut matches: Vec<String> = found
            .into_iter()
            .filter(|virtual_path| matches_tokens(&tokens, virtual_path.as_bytes()))
            .collect();
        matches.sort_unstable();
        Ok(matches)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        let real = self.resolve(path)?;
        let entries = fs::read_dir(&real).map_err(|err| map_io(path.as_str(), &err))?;
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|err| map_io(path.as_str(), &err))?;
            let file_name = entry.file_name();
            let name = utf8_name(&file_name, path.as_str())?;
            // DirEntry::metadata does not follow symlinks.
            let metadata = entry
                .metadata()
                .map_err(|err| map_io(path.as_str(), &err))?;
            result.push(Entry::new(name, stat_of(&metadata)));
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        let real = self.resolve_no_follow(path)?;
        let metadata = fs::symlink_metadata(&real).map_err(|err| map_io(path.as_str(), &err))?;
        Ok(stat_of(&metadata))
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let real = self.resolve_no_follow(path)?;
        if fs::symlink_metadata(&real).is_ok() {
            return Err(VfsError::AlreadyExists {
                path: path.to_string(),
            });
        }
        if recursive {
            fs::create_dir_all(&real)
        } else {
            fs::create_dir(&real)
        }
        .map_err(|err| map_io(path.as_str(), &err))
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(from)?;
        if from.as_str() == "/" {
            return Err(VfsError::RootProtected {
                path: from.to_string(),
            });
        }
        if to.as_str() == "/" {
            return Err(VfsError::RootProtected {
                path: to.to_string(),
            });
        }
        if to.as_str().starts_with(&format!("{}/", from.as_str())) {
            return Err(VfsError::InvalidPath {
                path: from.to_string(),
                reason: PathReason::IntoDescendant,
            });
        }
        let real_from = self.resolve_no_follow(from)?;
        let real_to = self.resolve_no_follow(to)?;
        // Validation finishes before the rename syscall, so a failed
        // rename changes nothing; the rename itself is atomic.
        fs::symlink_metadata(&real_from).map_err(|err| map_io(from.as_str(), &err))?;
        create_parent(&real_to, to)?;
        fs::rename(&real_from, &real_to).map_err(|err| map_io(from.as_str(), &err))
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(to)?;
        let real_from = self.resolve(from)?;
        let real_to = self.resolve(to)?;
        if real_from.is_dir() {
            return Err(VfsError::IsADirectory {
                path: from.to_string(),
            });
        }
        let bytes = fs::read(&real_from).map_err(|err| map_io(from.as_str(), &err))?;
        if real_to.is_dir() {
            return Err(VfsError::IsADirectory {
                path: to.to_string(),
            });
        }
        create_parent(&real_to, to)?;
        atomic_write(&real_to, to, &bytes)
    }
}
