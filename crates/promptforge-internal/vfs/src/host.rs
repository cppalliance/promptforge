//! The host filesystem backend, stage 1 (thin).
//!
//! [`HostBackend`] serves host-OS directories behind the virtual
//! namespace over direct `std::fs` calls. Two constructors:
//! [`HostBackend::identity`] (the virtual path IS the host path) and
//! [`HostBackend::rooted`] (chroot-style, with lexical plus
//! canonicalize containment). Writes, copies, and renames are
//! failure-atomic: a sibling temp file plus rename, so a failed
//! operation leaves source, destination, and accounting unchanged.
//!
//! Paths resolve two ways. Operations on a path itself (`remove`,
//! `exists`, `stat`, `mkdir`, `rename`) contain the parent and act on a
//! final-component link as a link, never its target. Operations on
//! contents (`read`, `read_range`, `write`, `append`, `list`, `glob`,
//! `copy`) follow links under the containment check, which denies a
//! link that resolves outside the root and refuses a path that passes
//! through a dangling link.
//!
//! Stage 2 hardening (the Bashkit RealFs resolver trio, symlink
//! policies, Windows long paths and device names) is deferred.

mod files;
mod resolve;
#[cfg(test)]
mod tests;

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::{PathReason, VfsError};
use crate::glob::{compile_glob, matches_tokens, validate_glob_pattern};
use crate::path::{VfsPath, canonicalize_absolute};
use crate::stat::{Entry, Stat};
use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

use files::{atomic_write, create_parent, is_dir_link, stat_of, walk, walk_root};
use resolve::{
    HostRoot, contain, contain_no_follow, identity_to_host, identity_to_virtual, join_virtual,
};

/// Maps an I/O failure to the error kind the trait surface promises.
/// Each `path` field holds the canonical path alone, so a host reading
/// the field per its documented contract gets a path, never the OS
/// error's text; the kind carries the OS failure, and only
/// `PermissionDenied` keeps the extra text in `reason`.
fn map_io(path: &str, err: &std::io::Error) -> VfsError {
    let message = format!("{path}: {err}");
    match err.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound {
            path: path.to_owned(),
        },
        std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied {
            path: path.to_owned(),
            reason: message,
        },
        std::io::ErrorKind::AlreadyExists => VfsError::AlreadyExists {
            path: path.to_owned(),
        },
        std::io::ErrorKind::IsADirectory => VfsError::IsADirectory {
            path: path.to_owned(),
        },
        std::io::ErrorKind::NotADirectory => VfsError::NotADirectory {
            path: path.to_owned(),
        },
        std::io::ErrorKind::DirectoryNotEmpty => VfsError::DirectoryNotEmpty {
            path: path.to_owned(),
        },
        _ => VfsError::Backend { message },
    }
}

/// A host filesystem backend behind the virtual namespace.
///
/// Stage 1 (thin): direct `std::fs` operations, lexical plus
/// canonicalize containment for [`HostBackend::rooted`], and
/// failure-atomic writes, copies, and renames. `ExecId` attribution is
/// accepted as a no-op: the host filesystem holds no per-identity
/// state.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HostBackend {
    root: HostRoot,
    read_only: bool,
}

impl HostBackend {
    /// A backend whose virtual paths ARE host paths: virtual
    /// `/a/b` is host `/a/b` (on Windows, virtual `/C:/a/b` is host
    /// `C:\a\b`). No containment applies.
    #[must_use]
    pub fn identity() -> HostBackend {
        HostBackend {
            root: HostRoot::Identity,
            read_only: false,
        }
    }

    /// A chroot-style backend: the virtual root is `dir`, canonicalized
    /// and validated as a directory at construction. Every resolved
    /// path is containment-checked against the canonical root.
    ///
    /// # Errors
    ///
    /// Returns [`VfsError::NotFound`] when `dir` is absent, or
    /// [`VfsError::NotADirectory`] when it is not a directory.
    pub fn rooted(dir: impl AsRef<Path>) -> Result<HostBackend, VfsError> {
        let display = dir.as_ref().to_string_lossy().into_owned();
        let canonical = fs::canonicalize(dir.as_ref()).map_err(|err| map_io(&display, &err))?;
        if !canonical.is_dir() {
            return Err(VfsError::NotADirectory { path: display });
        }
        Ok(HostBackend {
            root: HostRoot::Rooted(canonical),
            read_only: false,
        })
    }

    /// Sets whether the backend rejects all mutations. The flag is a
    /// property of the mount, orthogonal to policy.
    #[must_use]
    pub fn with_read_only(mut self, read_only: bool) -> HostBackend {
        self.read_only = read_only;
        self
    }
}

impl Vfs for HostBackend {
    fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError> {
        // Attribution is accepted as a no-op: the host filesystem holds
        // no per-identity state, and the claims model above the backend
        // enforces conflicts.
        let _ = cx;
        Ok(Box::new(HostAccess {
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

/// One identity's session with a [`HostBackend`]. The identity is
/// dropped on the floor: attribution is a no-op.
struct HostAccess {
    root: HostRoot,
    read_only: bool,
}

impl HostAccess {
    /// Resolves a canonical virtual path to its host path, applying
    /// containment in rooted mode.
    fn resolve(&self, path: &VfsPath) -> Result<PathBuf, VfsError> {
        match &self.root {
            HostRoot::Identity => Ok(identity_to_host(path.as_str())),
            HostRoot::Rooted(root) => {
                let candidate = join_virtual(root, path.as_str());
                contain(root, &candidate, path)
            }
        }
    }

    /// Resolves a canonical virtual path to its host path without
    /// following a final-component link, applying containment to the
    /// parent in rooted mode.
    fn resolve_no_follow(&self, path: &VfsPath) -> Result<PathBuf, VfsError> {
        match &self.root {
            HostRoot::Identity => Ok(identity_to_host(path.as_str())),
            HostRoot::Rooted(root) => {
                let candidate = join_virtual(root, path.as_str());
                contain_no_follow(root, &candidate, path)
            }
        }
    }

    /// Translates a host path back to its virtual spelling.
    fn to_virtual(&self, host: &Path) -> String {
        match &self.root {
            HostRoot::Identity => identity_to_virtual(host),
            HostRoot::Rooted(root) => {
                let relative = host.strip_prefix(root).unwrap_or(host);
                let mut virtual_path = String::new();
                for component in relative.components() {
                    virtual_path.push('/');
                    virtual_path.push_str(&component.as_os_str().to_string_lossy());
                }
                if virtual_path.is_empty() {
                    "/".to_owned()
                } else {
                    virtual_path
                }
            }
        }
    }

    /// Rejects mutations on a read-only backend before anything is
    /// touched: a denied operation never partially applies.
    fn check_writable(&self, path: &VfsPath) -> Result<(), VfsError> {
        if self.read_only {
            return Err(VfsError::PermissionDenied {
                path: path.to_string(),
                reason: format!("the host backend is read-only, so {path} cannot be mutated"),
            });
        }
        Ok(())
    }
}

impl VfsAccess for HostAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let host = self.resolve(path)?;
        if host.is_dir() {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        fs::read(&host).map_err(|err| map_io(path.as_str(), &err))
    }

    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        // Seek, never materialize: the host can position directly.
        let host = self.resolve(path)?;
        if host.is_dir() {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        let mut file = File::open(&host).map_err(|err| map_io(path.as_str(), &err))?;
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
        let host = self.resolve(path)?;
        if host.is_dir() {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        create_parent(&host, path)?;
        atomic_write(&host, contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let host = self.resolve(path)?;
        if host.is_dir() {
            return Err(VfsError::IsADirectory {
                path: path.to_string(),
            });
        }
        create_parent(&host, path)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&host)
            .map_err(|err| map_io(path.as_str(), &err))?;
        file.write_all(contents)
            .map_err(|err| map_io(path.as_str(), &err))
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.check_writable(path)?;
        if path.as_str() == "/" {
            return Err(VfsError::PermissionDenied {
                path: path.to_string(),
                reason: "the mounted root cannot be removed".into(),
            });
        }
        let host = self.resolve_no_follow(path)?;
        let metadata = fs::symlink_metadata(&host).map_err(|err| map_io(path.as_str(), &err))?;
        // symlink_metadata does not follow links: a symlink is removed
        // as a link, never its target.
        if metadata.is_dir() {
            if recursive {
                fs::remove_dir_all(&host)
            } else {
                fs::remove_dir(&host)
            }
        } else if is_dir_link(&metadata) {
            fs::remove_dir(&host)
        } else {
            fs::remove_file(&host)
        }
        .map_err(|err| map_io(path.as_str(), &err))
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        let host = self.resolve_no_follow(path)?;
        // symlink_metadata counts a dangling link as existing. Only a
        // confirmed absence is Ok(false); every other failure (a
        // denied permission, a genuine I/O error) surfaces as Err, as
        // the trait contract requires.
        match fs::symlink_metadata(&host) {
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
        let root = self.resolve(&canonicalize_absolute(walk_root(pattern))?)?;
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        walk(&root, &mut found)?;
        let mut matches: Vec<String> = found
            .iter()
            .map(|host| self.to_virtual(host))
            .filter(|virtual_path| matches_tokens(&tokens, virtual_path.as_bytes()))
            .collect();
        matches.sort_unstable();
        Ok(matches)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        let host = self.resolve(path)?;
        let entries = fs::read_dir(&host).map_err(|err| map_io(path.as_str(), &err))?;
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|err| map_io(path.as_str(), &err))?;
            // DirEntry::metadata does not follow symlinks.
            let metadata = entry
                .metadata()
                .map_err(|err| map_io(path.as_str(), &err))?;
            result.push(Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                stat: stat_of(&metadata),
                description: None,
            });
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        let host = self.resolve_no_follow(path)?;
        let metadata = fs::symlink_metadata(&host).map_err(|err| map_io(path.as_str(), &err))?;
        Ok(stat_of(&metadata))
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.check_writable(path)?;
        let host = self.resolve_no_follow(path)?;
        if fs::symlink_metadata(&host).is_ok() {
            return Err(VfsError::AlreadyExists {
                path: path.to_string(),
            });
        }
        if recursive {
            fs::create_dir_all(&host)
        } else {
            fs::create_dir(&host)
        }
        .map_err(|err| map_io(path.as_str(), &err))
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(from)?;
        if from.as_str() == "/" {
            return Err(VfsError::PermissionDenied {
                path: from.to_string(),
                reason: "the mounted root cannot be renamed".into(),
            });
        }
        if to.as_str() == "/" {
            return Err(VfsError::PermissionDenied {
                path: from.to_string(),
                reason: "a path cannot be renamed onto the mounted root".into(),
            });
        }
        if to.as_str().starts_with(&format!("{}/", from.as_str())) {
            return Err(VfsError::InvalidPath {
                path: from.to_string(),
                reason: PathReason::IntoDescendant,
            });
        }
        let host_from = self.resolve_no_follow(from)?;
        let host_to = self.resolve_no_follow(to)?;
        // Validation finishes before the rename syscall, so a failed
        // rename changes nothing; the rename itself is atomic.
        fs::symlink_metadata(&host_from).map_err(|err| map_io(from.as_str(), &err))?;
        create_parent(&host_to, to)?;
        fs::rename(&host_from, &host_to).map_err(|err| map_io(from.as_str(), &err))
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(to)?;
        let host_from = self.resolve(from)?;
        let host_to = self.resolve(to)?;
        if host_from.is_dir() {
            return Err(VfsError::IsADirectory {
                path: from.to_string(),
            });
        }
        let bytes = fs::read(&host_from).map_err(|err| map_io(from.as_str(), &err))?;
        if host_to.is_dir() {
            return Err(VfsError::IsADirectory {
                path: to.to_string(),
            });
        }
        create_parent(&host_to, to)?;
        atomic_write(&host_to, &bytes)
    }
}
