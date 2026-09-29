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

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{PathReason, VfsError};
use crate::glob::{compile_glob, matches_tokens, validate_glob_pattern};
use crate::path::{VfsPath, canonicalize_absolute};
use crate::stat::{Entry, FileType, Stat};
use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

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

/// How virtual paths reach host paths: verbatim, or contained under a
/// canonicalized root.
#[derive(Debug, Clone)]
enum HostRoot {
    /// The virtual path is the host path (modulo the Windows drive
    /// letter spelling).
    Identity,
    /// Chroot-style: the virtual root is this canonical host directory.
    Rooted(PathBuf),
}

/// Translates an identity-mode virtual path to a host path. On Windows
/// the virtual spelling of `C:\Users\x` is `/C:/Users/x`: a leading
/// slash before a drive letter is stripped.
#[cfg(windows)]
fn identity_to_host(virtual_path: &str) -> PathBuf {
    let bytes = virtual_path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return PathBuf::from(&virtual_path[1..]);
    }
    PathBuf::from(virtual_path)
}

/// Translates an identity-mode virtual path to a host path.
#[cfg(not(windows))]
fn identity_to_host(virtual_path: &str) -> PathBuf {
    PathBuf::from(virtual_path)
}

/// Translates a host path back to its identity-mode virtual spelling:
/// forward slashes, and on Windows a leading slash before a drive
/// letter (`C:\Users\x` becomes `/C:/Users/x`).
#[cfg(windows)]
fn identity_to_virtual(host: &Path) -> String {
    let spelled = host.to_string_lossy().replace('\\', "/");
    let bytes = spelled.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return format!("/{spelled}");
    }
    spelled
}

/// Translates a host path back to its identity-mode virtual spelling.
#[cfg(not(windows))]
fn identity_to_virtual(host: &Path) -> String {
    host.to_string_lossy().into_owned()
}

/// Joins a canonical virtual path onto a host root. The virtual path is
/// canonical (dot segments resolved at receipt, forward slashes), so
/// the join cannot escape lexically.
fn join_virtual(root: &Path, virtual_path: &str) -> PathBuf {
    let mut host = root.to_path_buf();
    for segment in virtual_path.split('/').filter(|s| !s.is_empty()) {
        host.push(segment);
    }
    host
}

/// Canonicalize containment: the candidate's nearest existing ancestor
/// is canonicalized and must sit under the (already canonical) root;
/// the missing tail is re-appended lexically. This catches link escapes
/// for existing paths while still resolving paths yet to be created.
/// A dangling link on the way up is refused: its target cannot be
/// canonicalized, and the host would follow it when creating.
fn contain(root: &Path, candidate: &Path, original: &VfsPath) -> Result<PathBuf, VfsError> {
    let denied = || VfsError::PermissionDenied {
        path: original.to_string(),
        reason: format!("{original} escapes the mounted root"),
    };
    let mut ancestor = candidate;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if ancestor.exists() {
            let canonical =
                fs::canonicalize(ancestor).map_err(|err| map_io(original.as_str(), &err))?;
            if !canonical.starts_with(root) {
                return Err(denied());
            }
            let mut resolved = canonical;
            for component in tail.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }
        if ancestor.symlink_metadata().is_ok() {
            return Err(VfsError::PermissionDenied {
                path: original.to_string(),
                reason: format!("{original} passes through a dangling symbolic link"),
            });
        }
        let Some(parent) = ancestor.parent() else {
            return Err(denied());
        };
        let Some(name) = ancestor.file_name() else {
            return Err(denied());
        };
        tail.push(name);
        ancestor = parent;
    }
}

/// No-follow containment: the candidate's parent is contained as in
/// [`contain`], so its nearest existing ancestor must sit under the
/// root, and the final component is appended unchanged, so a
/// final-component link is addressed as a link. The root itself
/// resolves to the root.
fn contain_no_follow(
    root: &Path,
    candidate: &Path,
    original: &VfsPath,
) -> Result<PathBuf, VfsError> {
    if candidate == root {
        return Ok(root.to_path_buf());
    }
    let (Some(parent), Some(name)) = (candidate.parent(), candidate.file_name()) else {
        return Err(VfsError::PermissionDenied {
            path: original.to_string(),
            reason: format!("{original} escapes the mounted root"),
        });
    };
    let mut resolved = contain(root, parent, original)?;
    resolved.push(name);
    Ok(resolved)
}

/// Uniquifies failure-atomic temp file names within the process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `contents` to `dest` failure-atomically: a sibling temp file
/// plus rename, so a failed write leaves the destination unchanged and
/// no temp file behind.
fn atomic_write(dest: &Path, contents: &[u8]) -> Result<(), VfsError> {
    let display = dest.to_string_lossy().into_owned();
    let Some(parent) = dest.parent() else {
        // A destination without a parent names only the filesystem root:
        // the write addresses nothing below the root. No canonical
        // virtual path produces this, so it is a backend impossibility,
        // not a path rule a caller broke; reporting a path reason here
        // would blame the path for the backend's own shape.
        return Err(VfsError::Backend {
            message: format!("write destination has no parent directory: {display:?}"),
        });
    };
    let Some(name) = dest.file_name() else {
        return Err(VfsError::Backend {
            message: format!("write destination has no file name: {display:?}"),
        });
    };
    let temp = parent.join(format!(
        ".{}.vfs-tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = (|| {
        let mut file = File::create(&temp).map_err(|err| map_io(&display, &err))?;
        file.write_all(contents)
            .map_err(|err| map_io(&display, &err))?;
        file.sync_all().map_err(|err| map_io(&display, &err))?;
        drop(file);
        fs::rename(&temp, dest).map_err(|err| map_io(&display, &err))
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome
}

/// Creates the destination's ancestor directories, matching the memory
/// backend's materialize-on-write semantics.
fn create_parent(host: &Path, path: &VfsPath) -> Result<(), VfsError> {
    if let Some(parent) = host.parent() {
        fs::create_dir_all(parent).map_err(|err| map_io(path.as_str(), &err))?;
    }
    Ok(())
}

/// The directory the walk must start from: the literal leading
/// segments of the pattern. A wildcard-free pattern can match only the
/// literal path itself, so the walk starts from its parent directory
/// for the literal file to be among the collected candidates, matching
/// the memory backend's parity.
fn walk_root(pattern: &str) -> &str {
    let literal = match pattern.find('*') {
        Some(star) => &pattern[..star],
        None => pattern,
    };
    match literal.rfind('/') {
        Some(0) | None => "/",
        Some(slash) => &literal[..slash],
    }
}

/// Collects every path under `dir`, files and directories, without
/// descending into links: a linked directory is collected, not
/// followed, so the walk can neither leave the mounted root nor loop.
fn walk(dir: &Path, found: &mut Vec<PathBuf>) -> Result<(), VfsError> {
    let entries = fs::read_dir(dir).map_err(|err| map_io(&dir.to_string_lossy(), &err))?;
    for entry in entries {
        let entry = entry.map_err(|err| map_io(&dir.to_string_lossy(), &err))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| map_io(&dir.to_string_lossy(), &err))?;
        found.push(path.clone());
        if file_type.is_dir() {
            walk(&path, found)?;
        }
    }
    Ok(())
}

/// Maps a host file type to the named POSIX kinds.
#[cfg(unix)]
fn file_type_of(file_type: fs::FileType) -> FileType {
    use std::os::unix::fs::FileTypeExt;
    if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else if file_type.is_file() {
        FileType::File
    } else if file_type.is_fifo() {
        FileType::Fifo
    } else if file_type.is_socket() {
        FileType::Socket
    } else if file_type.is_char_device() {
        FileType::CharDevice
    } else if file_type.is_block_device() {
        FileType::BlockDevice
    } else {
        FileType::File
    }
}

/// Maps a host file type to the named POSIX kinds. Windows distinguishes
/// only files, directories, and symlinks through `std`.
#[cfg(not(unix))]
fn file_type_of(file_type: fs::FileType) -> FileType {
    if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else {
        FileType::File
    }
}

/// Whether the entry is a directory link, a junction included, which
/// Windows removes with `remove_dir`: `remove_file` fails on one.
#[cfg(windows)]
fn is_dir_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::FileTypeExt;
    metadata.file_type().is_symlink_dir()
}

/// Whether the entry is a directory link needing `remove_dir`: never
/// off Windows, where `remove_file` removes any link.
#[cfg(not(windows))]
fn is_dir_link(_: &fs::Metadata) -> bool {
    false
}

/// POSIX mode bits where the host tracks them.
#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the not(unix) variant returns None; the Option unifies the platform signatures"
)]
fn mode_of(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

/// POSIX mode bits where the host tracks them: not on Windows.
#[cfg(not(unix))]
fn mode_of(_: &fs::Metadata) -> Option<u32> {
    None
}

/// Builds metadata without fabricating fields the host does not track.
fn stat_of(metadata: &fs::Metadata) -> Stat {
    Stat {
        file_type: file_type_of(metadata.file_type()),
        size: metadata.len(),
        mode: mode_of(metadata),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{HostBackend, atomic_write, identity_to_virtual, map_io};
    use crate::error::{PathReason, VfsError};
    use crate::handle::Scope;
    use crate::path::{VfsPath, canonicalize_absolute};
    use crate::stat::FileType;
    use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

    fn path(s: &str) -> Result<VfsPath, VfsError> {
        canonicalize_absolute(s)
    }

    /// A fresh identity in a fresh scope, for acquiring the backend
    /// directly.
    fn context() -> AcquireContext {
        AcquireContext::new(ExecId::vend(), Scope::start())
    }

    /// A unique temporary directory that removes itself on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Result<TempDir, VfsError> {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "promptforge-vfs-host-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).map_err(|err| map_io("the temporary directory", &err))?;
            Ok(TempDir(dir))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Acquires a session on a rooted backend over `dir`.
    fn rooted_access(dir: &Path) -> Result<Box<dyn VfsAccess>, VfsError> {
        let mut backend = HostBackend::rooted(dir)?;
        backend.acquire(&context())
    }

    /// Asserts no failure-atomic temp file survived under `dir`.
    fn assert_no_temp_files_left(dir: &Path) -> Result<(), VfsError> {
        for entry in fs::read_dir(dir).map_err(|err| map_io("listing the temp dir", &err))? {
            let entry = entry.map_err(|err| map_io("listing the temp dir", &err))?;
            assert!(
                !entry.file_name().to_string_lossy().contains(".vfs-tmp-"),
                "a temp file survived: {}",
                entry.path().display()
            );
        }
        Ok(())
    }

    /// Creates a directory link, returning false when the host refuses.
    /// Windows uses a junction (no privilege required, unlike
    /// `symlink_dir`); Unix uses a plain symlink.
    #[cfg(windows)]
    fn make_dir_link(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Creates a directory link, returning false when the host refuses.
    #[cfg(unix)]
    fn make_dir_link(link: &Path, target: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// Creates a file link, returning false only when Windows refuses
    /// for want of the symlink privilege (raw OS error 1314,
    /// `ERROR_PRIVILEGE_NOT_HELD`). Every other failure is an error.
    #[cfg(windows)]
    fn make_file_link(link: &Path, target: &Path) -> Result<bool, VfsError> {
        const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
        match std::os::windows::fs::symlink_file(target, link) {
            Ok(()) => Ok(true),
            Err(err) if err.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
                eprintln!(
                    "skipped: Windows refused a file symlink without the symlink privilege \
                     (enable Developer Mode or run elevated)"
                );
                Ok(false)
            }
            Err(err) => Err(map_io("creating the file link", &err)),
        }
    }

    /// Creates a file link.
    #[cfg(unix)]
    fn make_file_link(link: &Path, target: &Path) -> Result<bool, VfsError> {
        std::os::unix::fs::symlink(target, link)
            .map_err(|err| map_io("creating the file link", &err))?;
        Ok(true)
    }

    /// Whether `host` itself is a link, without following it.
    fn is_link(host: &Path) -> bool {
        fs::symlink_metadata(host).is_ok_and(|metadata| metadata.file_type().is_symlink())
    }

    /// Makes `link` a dangling directory link: a link to `target`, which
    /// is then removed. A directory link needs no privilege on any host.
    fn make_dangling_dir_link(link: &Path, target: &Path) -> Result<(), VfsError> {
        fs::create_dir(target).map_err(|err| map_io("creating the link target", &err))?;
        assert!(
            make_dir_link(link, target),
            "the directory link must be created"
        );
        fs::remove_dir(target).map_err(|err| map_io("removing the link target", &err))
    }

    /// Asserts `result` is the dangling-link refusal, not some other
    /// denial or an OS failure.
    fn assert_dangling_refusal<T: std::fmt::Debug>(result: Result<T, VfsError>, operation: &str) {
        match result {
            Err(VfsError::PermissionDenied { reason, .. }) => assert!(
                reason.contains("passes through a dangling symbolic link"),
                "{operation}: {reason}"
            ),
            other => panic!("{operation} must be refused as a dangling link, got {other:?}"),
        }
    }

    #[test]
    fn a_rooted_backend_round_trips_files_and_directories() -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        let mut access = rooted_access(temp.path())?;
        // Writes materialize ancestor directories, as the memory
        // backend does: no mkdir is needed first.
        access.write(&path("/a/b/f.txt")?, b"hello")?;
        assert_eq!(access.read(&path("/a/b/f.txt")?)?, b"hello");
        access.append(&path("/a/b/f.txt")?, b" world")?;
        assert_eq!(access.read(&path("/a/b/f.txt")?)?, b"hello world");
        // The seek override serves byte ranges without a whole read.
        assert_eq!(access.read_range(&path("/a/b/f.txt")?, 6, 5)?, b"world");
        assert!(access.exists(&path("/a")?)?);
        assert!(access.exists(&path("/a/b")?)?);
        assert!(!access.exists(&path("/a/missing.txt")?)?);
        let stat = access.stat(&path("/a/b/f.txt")?)?;
        assert_eq!(stat.file_type, FileType::File);
        assert_eq!(stat.size, 11);
        // The host tracks modification times; honesty permits Some here.
        assert!(stat.modified.is_some());
        let entries = access.list(&path("/a/b")?)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "f.txt");
        assert_eq!(entries[0].stat.file_type, FileType::File);
        assert!(entries[0].description.is_none());
        access.mkdir(&path("/a/c")?, false)?;
        assert!(matches!(
            access.mkdir(&path("/a/c")?, false),
            Err(VfsError::AlreadyExists { .. })
        ));
        access.rename(&path("/a/b/f.txt")?, &path("/a/b/g.txt")?)?;
        assert!(!access.exists(&path("/a/b/f.txt")?)?);
        access.copy(&path("/a/b/g.txt")?, &path("/a/c/h.txt")?)?;
        assert_eq!(access.read(&path("/a/c/h.txt")?)?, b"hello world");
        assert_eq!(
            access.glob("/a/**/*.txt")?,
            vec!["/a/b/g.txt".to_owned(), "/a/c/h.txt".to_owned()]
        );
        // The mounted root itself cannot be removed.
        assert!(matches!(
            access.remove(&path("/")?, true),
            Err(VfsError::PermissionDenied { .. })
        ));
        access.remove(&path("/a")?, true)?;
        assert!(!access.exists(&path("/a")?)?);
        Ok(())
    }

    #[test]
    fn a_rooted_backend_rejects_links_that_escape_the_mount_root() -> Result<(), VfsError> {
        let outside = TempDir::new()?;
        fs::write(outside.path().join("secret.txt"), b"classified")
            .map_err(|err| map_io("seeding the outside file", &err))?;
        let root = TempDir::new()?;
        if !make_dir_link(&root.path().join("link"), outside.path()) {
            eprintln!(
                "skipped: the host refused to create a directory link, so there is no link \
                 to escape through"
            );
            return Ok(());
        }
        let mut access = rooted_access(root.path())?;
        assert!(
            matches!(
                access.read(&path("/link/secret.txt")?),
                Err(VfsError::PermissionDenied { .. })
            ),
            "a read through the escaping link must be denied"
        );
        assert!(
            matches!(
                access.write(&path("/link/new.txt")?, b"x"),
                Err(VfsError::PermissionDenied { .. })
            ),
            "a write through the escaping link must be denied"
        );
        assert!(
            matches!(
                access.append(&path("/link/secret.txt")?, b"x"),
                Err(VfsError::PermissionDenied { .. })
            ),
            "an append through the escaping link must be denied"
        );
        assert_eq!(
            fs::read(outside.path().join("secret.txt"))
                .map_err(|err| map_io("reading the outside file", &err))?,
            b"classified"
        );
        assert!(!outside.path().join("new.txt").exists());
        Ok(())
    }

    #[test]
    fn removing_a_link_to_an_in_root_file_removes_the_link_and_keeps_the_target()
    -> Result<(), VfsError> {
        let root = TempDir::new()?;
        fs::write(root.path().join("target.txt"), b"kept")
            .map_err(|err| map_io("seeding the target file", &err))?;
        if !make_file_link(&root.path().join("link"), &root.path().join("target.txt"))? {
            return Ok(());
        }
        let mut access = rooted_access(root.path())?;
        access.remove(&path("/link")?, false)?;
        assert!(!is_link(&root.path().join("link")), "the link must be gone");
        assert_eq!(access.read(&path("/target.txt")?)?, b"kept");
        Ok(())
    }

    #[test]
    fn path_operations_act_on_a_link_to_an_outside_file_as_a_link() -> Result<(), VfsError> {
        let outside = TempDir::new()?;
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, b"classified")
            .map_err(|err| map_io("seeding the outside file", &err))?;
        let root = TempDir::new()?;
        if !make_file_link(&root.path().join("link"), &secret)? {
            return Ok(());
        }
        let mut access = rooted_access(root.path())?;
        assert!(access.exists(&path("/link")?)?);
        assert_eq!(access.stat(&path("/link")?)?.file_type, FileType::Symlink);
        assert!(matches!(
            access.mkdir(&path("/link")?, false),
            Err(VfsError::AlreadyExists { .. })
        ));
        assert!(
            matches!(
                access.read(&path("/link")?),
                Err(VfsError::PermissionDenied { .. })
            ),
            "a read through the escaping link must be denied"
        );
        assert!(
            matches!(
                access.write(&path("/link")?, b"x"),
                Err(VfsError::PermissionDenied { .. })
            ),
            "a write through the escaping link must be denied"
        );
        access.rename(&path("/link")?, &path("/moved")?)?;
        assert!(!access.exists(&path("/link")?)?);
        assert!(
            is_link(&root.path().join("moved")),
            "the link itself must move"
        );
        assert!(
            secret.is_file(),
            "the outside target must stay where it was"
        );
        access.remove(&path("/moved")?, false)?;
        assert!(!access.exists(&path("/moved")?)?);
        assert_eq!(
            fs::read(&secret).map_err(|err| map_io("reading the outside file", &err))?,
            b"classified"
        );
        Ok(())
    }

    #[test]
    fn removing_a_dangling_link_succeeds() -> Result<(), VfsError> {
        let root = TempDir::new()?;
        if !make_file_link(
            &root.path().join("dangling"),
            &root.path().join("missing.txt"),
        )? {
            return Ok(());
        }
        let mut access = rooted_access(root.path())?;
        access.remove(&path("/dangling")?, false)?;
        assert!(
            !is_link(&root.path().join("dangling")),
            "the link must be gone"
        );
        Ok(())
    }

    #[test]
    fn content_operations_refuse_a_path_through_a_dangling_link() -> Result<(), VfsError> {
        let outside = TempDir::new()?;
        for target_in_root in [true, false] {
            let root = TempDir::new()?;
            let target = if target_in_root {
                root.path().join("gone")
            } else {
                outside.path().join("gone")
            };
            make_dangling_dir_link(&root.path().join("link"), &target)?;
            let mut access = rooted_access(root.path())?;
            for spelled in ["/link", "/link/new.txt"] {
                let at = path(spelled)?;
                assert_dangling_refusal(access.append(&at, b"x"), &format!("append {spelled}"));
                assert_dangling_refusal(access.write(&at, b"x"), &format!("write {spelled}"));
                assert_dangling_refusal(access.read(&at), &format!("read {spelled}"));
                assert_dangling_refusal(access.list(&at), &format!("list {spelled}"));
            }
            assert!(
                fs::symlink_metadata(&target).is_err(),
                "nothing may appear at the link's target {}",
                target.display()
            );
        }
        Ok(())
    }

    #[test]
    fn path_operations_act_on_a_dangling_link_itself_and_refuse_a_path_through_it()
    -> Result<(), VfsError> {
        let root = TempDir::new()?;
        let link = root.path().join("link");
        make_dangling_dir_link(&link, &root.path().join("gone"))?;
        let mut access = rooted_access(root.path())?;
        assert!(access.exists(&path("/link")?)?, "the link itself exists");
        assert_dangling_refusal(
            access.exists(&path("/link/new.txt")?),
            "exists /link/new.txt",
        );
        assert_dangling_refusal(access.mkdir(&path("/link/sub")?, false), "mkdir /link/sub");
        access.remove(&path("/link")?, false)?;
        assert!(!is_link(&link), "the link must be gone");
        assert!(!access.exists(&path("/link")?)?);
        Ok(())
    }

    #[test]
    fn removing_a_directory_link_keeps_the_target_directory_and_its_contents()
    -> Result<(), VfsError> {
        let root = TempDir::new()?;
        let target = root.path().join("real");
        fs::create_dir(&target).map_err(|err| map_io("creating the target directory", &err))?;
        fs::write(target.join("keep.txt"), b"kept")
            .map_err(|err| map_io("seeding the target file", &err))?;
        let mut access = rooted_access(root.path())?;
        for recursive in [false, true] {
            let link = root.path().join("dirlink");
            assert!(
                make_dir_link(&link, &target),
                "the directory link must be created"
            );
            access.remove(&path("/dirlink")?, recursive)?;
            assert!(
                !is_link(&link),
                "the link must be gone (recursive: {recursive})"
            );
            assert_eq!(access.read(&path("/real/keep.txt")?)?, b"kept");
        }
        Ok(())
    }

    #[test]
    fn path_operations_act_on_a_directory_link_to_an_outside_directory_as_a_link()
    -> Result<(), VfsError> {
        let outside = TempDir::new()?;
        let secret = outside.path().join("secret.txt");
        fs::write(&secret, b"classified")
            .map_err(|err| map_io("seeding the outside file", &err))?;
        let root = TempDir::new()?;
        assert!(
            make_dir_link(&root.path().join("link"), outside.path()),
            "the directory link must be created"
        );
        let mut access = rooted_access(root.path())?;
        assert!(access.exists(&path("/link")?)?);
        assert_eq!(access.stat(&path("/link")?)?.file_type, FileType::Symlink);
        access.rename(&path("/link")?, &path("/moved")?)?;
        assert!(!access.exists(&path("/link")?)?);
        assert!(
            is_link(&root.path().join("moved")),
            "the link itself must move"
        );
        access.remove(&path("/moved")?, true)?;
        assert!(!access.exists(&path("/moved")?)?);
        assert_eq!(
            fs::read(&secret).map_err(|err| map_io("reading the outside file", &err))?,
            b"classified"
        );
        Ok(())
    }

    #[test]
    fn a_failed_write_leaves_the_destination_unchanged_and_no_temp_file_behind()
    -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        let mut access = rooted_access(temp.path())?;
        // A write over an existing directory fails before the temp
        // file is created; the directory survives.
        access.mkdir(&path("/dir")?, false)?;
        assert!(matches!(
            access.write(&path("/dir")?, b"x"),
            Err(VfsError::IsADirectory { .. })
        ));
        assert!(temp.path().join("dir").is_dir());
        // A write through a file ancestor fails; the file is unchanged.
        access.write(&path("/f.txt")?, b"original")?;
        assert!(access.write(&path("/f.txt/g.txt")?, b"x").is_err());
        assert_eq!(access.read(&path("/f.txt")?)?, b"original");
        assert_no_temp_files_left(temp.path())?;
        Ok(())
    }

    #[test]
    fn a_failed_copy_leaves_source_and_destination_unchanged() -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        let mut access = rooted_access(temp.path())?;
        access.write(&path("/dst.txt")?, b"old")?;
        assert!(matches!(
            access.copy(&path("/missing.txt")?, &path("/dst.txt")?),
            Err(VfsError::NotFound { .. })
        ));
        assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
        assert_no_temp_files_left(temp.path())?;
        Ok(())
    }

    #[test]
    fn a_failed_rename_leaves_source_and_destination_unchanged() -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        let mut access = rooted_access(temp.path())?;
        access.write(&path("/dst.txt")?, b"old")?;
        assert!(matches!(
            access.rename(&path("/missing.txt")?, &path("/dst.txt")?),
            Err(VfsError::NotFound { .. })
        ));
        assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
        // Renaming a directory into its own descendant is rejected, and
        // the rejection names the descendant rule.
        access.mkdir(&path("/d")?, false)?;
        assert_eq!(
            access.rename(&path("/d")?, &path("/d/inner")?),
            Err(VfsError::InvalidPath {
                path: "/d".to_owned(),
                reason: PathReason::IntoDescendant,
            })
        );
        assert!(access.exists(&path("/d")?)?);
        assert_no_temp_files_left(temp.path())?;
        Ok(())
    }

    #[test]
    fn a_read_only_backend_rejects_every_mutation_but_serves_reads() -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        fs::write(temp.path().join("keep.txt"), b"keep")
            .map_err(|err| map_io("seeding the kept file", &err))?;
        let mut backend = HostBackend::rooted(temp.path())?.with_read_only(true);
        assert!(Vfs::read_only(&backend));
        let mut access = backend.acquire(&context())?;
        assert_eq!(access.read(&path("/keep.txt")?)?, b"keep");
        assert!(access.exists(&path("/keep.txt")?)?);
        assert_eq!(access.stat(&path("/keep.txt")?)?.size, 4);
        for result in [
            access.write(&path("/keep.txt")?, b"x"),
            access.write(&path("/new.txt")?, b"x"),
            access.append(&path("/keep.txt")?, b"x"),
            access.remove(&path("/keep.txt")?, false),
            access.mkdir(&path("/dir")?, false),
            access.rename(&path("/keep.txt")?, &path("/moved.txt")?),
            access.copy(&path("/keep.txt")?, &path("/copy.txt")?),
        ] {
            assert!(
                matches!(result, Err(VfsError::PermissionDenied { .. })),
                "expected a read-only denial, got {result:?}"
            );
        }
        assert_eq!(access.read(&path("/keep.txt")?)?, b"keep");
        assert!(!temp.path().join("new.txt").exists());
        Ok(())
    }

    #[test]
    fn an_identity_backend_maps_virtual_paths_directly_to_host_paths() -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        let host_file = temp.path().join("identity.txt");
        let virtual_spelling = identity_to_virtual(&host_file);
        let mut backend = HostBackend::identity();
        let mut access = backend.acquire(&context())?;
        access.write(&path(&virtual_spelling)?, b"direct")?;
        assert_eq!(
            fs::read(&host_file).map_err(|err| map_io("reading the host file", &err))?,
            b"direct"
        );
        assert_eq!(access.read(&path(&virtual_spelling)?)?, b"direct");
        assert!(access.exists(&path(&virtual_spelling)?)?);
        Ok(())
    }

    #[test]
    fn a_literal_glob_pattern_matches_the_file_itself() -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        let mut access = rooted_access(temp.path())?;
        access.write(&path("/d/a.txt")?, b"x")?;
        // A wildcard-free pattern matches the literal path itself, at
        // parity with the memory backend.
        assert_eq!(access.glob("/d/a.txt")?, vec!["/d/a.txt".to_owned()]);
        assert_eq!(access.glob("/d/missing.txt")?, Vec::<String>::new());
        // A literal directory matches itself as well.
        assert_eq!(access.glob("/d")?, vec!["/d".to_owned()]);
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn exists_surfaces_backend_failures_as_errors() -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        let mut access = rooted_access(temp.path())?;
        access.write(&path("/f.txt")?, b"x")?;
        // A lookup through a file ancestor fails with ENOTDIR, not
        // ENOENT: an indeterminate path must not report as absent.
        assert!(matches!(
            access.exists(&path("/f.txt/g.txt")?),
            Err(VfsError::NotADirectory { .. })
        ));
        assert!(!access.exists(&path("/missing.txt")?)?);
        Ok(())
    }

    #[test]
    fn the_rooted_constructor_requires_an_existing_directory() -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        fs::write(temp.path().join("f.txt"), b"x")
            .map_err(|err| map_io("seeding the file", &err))?;
        assert!(matches!(
            HostBackend::rooted(temp.path().join("f.txt")),
            Err(VfsError::NotADirectory { .. })
        ));
        assert!(matches!(
            HostBackend::rooted(temp.path().join("missing")),
            Err(VfsError::NotFound { .. })
        ));
        Ok(())
    }

    #[test]
    fn map_io_reports_the_canonical_path_not_the_os_sentence() {
        use std::io::ErrorKind;
        // The kind carries the OS failure; the `path` field holds the
        // canonical path alone, as the field's documented contract
        // promises, so a host reading it gets a path, never OS text.
        let cases = [
            (ErrorKind::NotFound, "not found: /x"),
            (ErrorKind::AlreadyExists, "already exists: /x"),
            (ErrorKind::IsADirectory, "is a directory: /x"),
            (ErrorKind::NotADirectory, "not a directory: /x"),
            (ErrorKind::DirectoryNotEmpty, "directory not empty: /x"),
        ];
        for (kind, text) in cases {
            assert_eq!(
                map_io("/x", &std::io::Error::from(kind)).to_string(),
                text,
                "the OS detail must not crowd out the path"
            );
        }
        // PermissionDenied keeps the OS text beside the path, in its
        // reason field.
        match map_io("/x", &std::io::Error::from(ErrorKind::PermissionDenied)) {
            VfsError::PermissionDenied { path, reason } => {
                assert_eq!(path, "/x");
                assert!(reason.contains("/x"), "{reason}");
            }
            other => panic!("expected a permission denial, got {other:?}"),
        }
    }

    #[test]
    fn an_atomic_write_addressing_only_a_root_is_a_backend_failure() {
        // A destination without a parent or a file name names nothing
        // below the root; it is a backend impossibility, not a path rule
        // a caller broke, so it must not render as "path is empty".
        let err = atomic_write(Path::new("/"), b"x").unwrap_err();
        assert!(
            matches!(err, VfsError::Backend { .. }),
            "a root-only destination is not a path-validation failure: {err:?}"
        );
    }

    /// The rooted-path, idempotent-remove, and split-glob semantics of
    /// the public capability, exercised over the host backend.
    mod semantics {
        use super::{HostBackend, TempDir, VfsError};
        use crate::{Origin, PathReason, VfsRef};

        fn rooted(temp: &TempDir) -> Result<VfsRef, VfsError> {
            Ok(VfsRef::new(HostBackend::rooted(temp.path())?))
        }

        #[test]
        fn a_relative_path_joins_onto_the_access_root() -> Result<(), VfsError> {
            let temp = TempDir::new()?;
            let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
            access.write("a/b.txt", b"hi")?;
            assert_eq!(access.read("a/b.txt")?, b"hi");
            assert_eq!(access.read("/a/b.txt")?, b"hi");
            Ok(())
        }

        #[test]
        fn dotdot_resolves_inside_the_access_root_and_is_refused_above_it() -> Result<(), VfsError>
        {
            let temp = TempDir::new()?;
            let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
            access.write("/drafts/f.txt", b"x")?;
            assert_eq!(access.read("drafts/../drafts/f.txt")?, b"x");
            assert!(matches!(
                access.read("../f.txt"),
                Err(VfsError::InvalidPath { .. })
            ));
            Ok(())
        }

        #[test]
        fn removing_a_missing_path_is_ok_false() -> Result<(), VfsError> {
            let temp = TempDir::new()?;
            let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
            assert!(!access.remove("missing.txt", false)?);
            access.write("f.txt", b"x")?;
            assert!(access.remove("f.txt", false)?);
            assert!(!access.remove("f.txt", false)?);
            Ok(())
        }

        #[test]
        fn glob_returns_files_and_a_trailing_slash_selects_directories() -> Result<(), VfsError> {
            let temp = TempDir::new()?;
            let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
            access.write("/d/a.txt", b"")?;
            access.write("/d/sub/c.txt", b"")?;
            assert_eq!(access.glob("/d/*")?, vec!["/d/a.txt".to_owned()]);
            assert_eq!(access.glob("/d/*/")?, vec!["/d/sub".to_owned()]);
            Ok(())
        }

        #[test]
        fn a_relative_pattern_yields_relative_results() -> Result<(), VfsError> {
            let temp = TempDir::new()?;
            let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
            access.write("/d/a.txt", b"")?;
            assert_eq!(access.glob("d/*.txt")?, vec!["d/a.txt".to_owned()]);
            Ok(())
        }

        #[test]
        fn glob_refuses_a_backslash_in_the_raw_pattern() -> Result<(), VfsError> {
            let temp = TempDir::new()?;
            let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
            assert_eq!(
                access.glob("/a\\b"),
                Err(VfsError::InvalidPath {
                    path: "/a\\b".to_owned(),
                    reason: PathReason::Backslash,
                })
            );
            Ok(())
        }

        #[test]
        fn glob_refuses_control_characters_and_bad_grammar_with_their_reasons()
        -> Result<(), VfsError> {
            let temp = TempDir::new()?;
            let access = rooted(&temp)?.acquire(Origin::new("host semantics test"))?;
            assert_eq!(
                access.glob("/a\u{0}b"),
                Err(VfsError::InvalidPath {
                    path: "/a\u{0}b".to_owned(),
                    reason: PathReason::Control,
                })
            );
            assert_eq!(
                access.glob("/a/***/b"),
                Err(VfsError::InvalidPath {
                    path: "/a/***/b".to_owned(),
                    reason: PathReason::Wildcard,
                })
            );
            Ok(())
        }
    }
}
