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
//! Stage 2 hardening (the Bashkit RealFs resolver trio, symlink
//! policies, Windows long paths and device names) is deferred. The
//! known stage 1 limitation: containment canonicalizes the nearest
//! existing ancestor, so a dangling symlink inside the root is not
//! itself resolved; writing through one follows the host's own
//! symlink semantics.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::VfsError;
use crate::glob::{MAX_GLOB_PATTERN_BYTES, compile_glob, matches_tokens, validate_glob_grammar};
use crate::path::{VfsPath, canonicalize};
use crate::traits::{ExecId, Vfs, VfsAccess};
use crate::types::{Entry, FileType, Stat};

/// Maps an I/O failure to the error kind the trait surface promises.
fn map_io(path: &str, err: &std::io::Error) -> VfsError {
    let message = format!("{path}: {err}");
    match err.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound(message),
        std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied(message),
        std::io::ErrorKind::AlreadyExists => VfsError::AlreadyExists(message),
        std::io::ErrorKind::IsADirectory => VfsError::IsADirectory(message),
        std::io::ErrorKind::NotADirectory => VfsError::NotADirectory(message),
        std::io::ErrorKind::DirectoryNotEmpty => VfsError::DirectoryNotEmpty(message),
        _ => VfsError::Backend(message),
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
fn contain(root: &Path, candidate: &Path, original: VfsPath) -> Result<PathBuf, VfsError> {
    let denied = || VfsError::PermissionDenied(format!("{original} escapes the mounted root"));
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

/// Uniquifies failure-atomic temp file names within the process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `contents` to `dest` failure-atomically: a sibling temp file
/// plus rename, so a failed write leaves the destination unchanged and
/// no temp file behind.
fn atomic_write(dest: &Path, contents: &[u8]) -> Result<(), VfsError> {
    let display = dest.to_string_lossy().into_owned();
    let Some(parent) = dest.parent() else {
        return Err(VfsError::InvalidPath(format!(
            "{display} has no parent directory"
        )));
    };
    let Some(name) = dest.file_name() else {
        return Err(VfsError::InvalidPath(format!("{display} has no file name")));
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
fn create_parent(host: &Path, path: VfsPath) -> Result<(), VfsError> {
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

/// POSIX mode bits where the host tracks them.
#[cfg(unix)]
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
            return Err(VfsError::NotADirectory(display));
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
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError> {
        // Attribution is accepted as a no-op: the host filesystem holds
        // no per-identity state, and the claims model above the backend
        // enforces conflicts.
        let _ = id;
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
    fn resolve(&self, path: VfsPath) -> Result<PathBuf, VfsError> {
        match &self.root {
            HostRoot::Identity => Ok(identity_to_host(path.as_str())),
            HostRoot::Rooted(root) => {
                let candidate = join_virtual(root, path.as_str());
                contain(root, &candidate, path)
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
    fn check_writable(&self, path: VfsPath) -> Result<(), VfsError> {
        if self.read_only {
            return Err(VfsError::PermissionDenied(format!(
                "the host backend is read-only, so {path} cannot be mutated"
            )));
        }
        Ok(())
    }
}

impl VfsAccess for HostAccess {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let host = self.resolve(*path)?;
        if host.is_dir() {
            return Err(VfsError::IsADirectory(path.to_string()));
        }
        fs::read(&host).map_err(|err| map_io(path.as_str(), &err))
    }

    fn read_range(&self, path: &VfsPath, offset: u64, len: u64) -> Result<Vec<u8>, VfsError> {
        // Seek, never materialize: the host can position directly.
        let host = self.resolve(*path)?;
        if host.is_dir() {
            return Err(VfsError::IsADirectory(path.to_string()));
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
        self.check_writable(*path)?;
        let host = self.resolve(*path)?;
        if host.is_dir() {
            return Err(VfsError::IsADirectory(path.to_string()));
        }
        create_parent(&host, *path)?;
        atomic_write(&host, contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.check_writable(*path)?;
        let host = self.resolve(*path)?;
        if host.is_dir() {
            return Err(VfsError::IsADirectory(path.to_string()));
        }
        create_parent(&host, *path)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&host)
            .map_err(|err| map_io(path.as_str(), &err))?;
        file.write_all(contents)
            .map_err(|err| map_io(path.as_str(), &err))
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.check_writable(*path)?;
        if path.as_str() == "/" {
            return Err(VfsError::PermissionDenied(
                "the mounted root cannot be removed".into(),
            ));
        }
        let host = self.resolve(*path)?;
        let metadata = fs::symlink_metadata(&host).map_err(|err| map_io(path.as_str(), &err))?;
        // symlink_metadata does not follow links: a symlink is removed
        // as a link, never its target.
        if metadata.is_dir() {
            if recursive {
                fs::remove_dir_all(&host)
            } else {
                fs::remove_dir(&host)
            }
        } else {
            fs::remove_file(&host)
        }
        .map_err(|err| map_io(path.as_str(), &err))
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        let host = self.resolve(*path)?;
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
        if pattern.len() > MAX_GLOB_PATTERN_BYTES {
            return Err(VfsError::InvalidPath(format!(
                "glob pattern exceeds {MAX_GLOB_PATTERN_BYTES} bytes"
            )));
        }
        if let Err(reason) = validate_glob_grammar(pattern) {
            return Err(VfsError::InvalidPath(format!(
                "invalid glob pattern {pattern:?}: {reason}"
            )));
        }
        let tokens = compile_glob(pattern.as_bytes());
        let root = self.resolve(canonicalize(walk_root(pattern))?)?;
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
        let host = self.resolve(*path)?;
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
        let host = self.resolve(*path)?;
        let metadata = fs::symlink_metadata(&host).map_err(|err| map_io(path.as_str(), &err))?;
        Ok(stat_of(&metadata))
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.check_writable(*path)?;
        let host = self.resolve(*path)?;
        if fs::symlink_metadata(&host).is_ok() {
            return Err(VfsError::AlreadyExists(path.to_string()));
        }
        if recursive {
            fs::create_dir_all(&host)
        } else {
            fs::create_dir(&host)
        }
        .map_err(|err| map_io(path.as_str(), &err))
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(*from)?;
        if from.as_str() == "/" {
            return Err(VfsError::PermissionDenied(
                "the mounted root cannot be renamed".into(),
            ));
        }
        if to.as_str() == "/" {
            return Err(VfsError::PermissionDenied(
                "a path cannot be renamed onto the mounted root".into(),
            ));
        }
        if to.as_str().starts_with(&format!("{}/", from.as_str())) {
            return Err(VfsError::InvalidPath(format!(
                "cannot rename {from} into its own descendant {to}"
            )));
        }
        let host_from = self.resolve(*from)?;
        let host_to = self.resolve(*to)?;
        // Validation finishes before the rename syscall, so a failed
        // rename changes nothing; the rename itself is atomic.
        fs::symlink_metadata(&host_from).map_err(|err| map_io(from.as_str(), &err))?;
        create_parent(&host_to, *to)?;
        fs::rename(&host_from, &host_to).map_err(|err| map_io(from.as_str(), &err))
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.check_writable(*to)?;
        let host_from = self.resolve(*from)?;
        let host_to = self.resolve(*to)?;
        if host_from.is_dir() {
            return Err(VfsError::IsADirectory(from.to_string()));
        }
        let bytes = fs::read(&host_from).map_err(|err| map_io(from.as_str(), &err))?;
        if host_to.is_dir() {
            return Err(VfsError::IsADirectory(to.to_string()));
        }
        create_parent(&host_to, *to)?;
        atomic_write(&host_to, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{HostBackend, identity_to_virtual, map_io};
    use crate::error::VfsError;
    use crate::path::{VfsPath, canonicalize};
    use crate::traits::{ExecId, Vfs, VfsAccess};
    use crate::types::FileType;

    fn path(s: &str) -> Result<VfsPath, VfsError> {
        canonicalize(s)
    }

    /// A unique temporary directory that removes itself on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Result<TempDir, VfsError> {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "shared-vfs-host-test-{}-{}",
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
        backend.acquire(ExecId::vend())
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
            Err(VfsError::AlreadyExists(_))
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
            Err(VfsError::PermissionDenied(_))
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
            // The host refused the link (privileges); there is nothing
            // to escape through, so the test vacuously passes.
            return Ok(());
        }
        let mut access = rooted_access(root.path())?;
        assert!(
            matches!(
                access.read(&path("/link/secret.txt")?),
                Err(VfsError::PermissionDenied(_))
            ),
            "a read through the escaping link must be denied"
        );
        assert!(
            matches!(
                access.write(&path("/link/new.txt")?, b"x"),
                Err(VfsError::PermissionDenied(_))
            ),
            "a write through the escaping link must be denied"
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
    fn a_failed_write_leaves_the_destination_unchanged_and_no_temp_file_behind()
    -> Result<(), VfsError> {
        let temp = TempDir::new()?;
        let mut access = rooted_access(temp.path())?;
        // A write over an existing directory fails before the temp
        // file is created; the directory survives.
        access.mkdir(&path("/dir")?, false)?;
        assert!(matches!(
            access.write(&path("/dir")?, b"x"),
            Err(VfsError::IsADirectory(_))
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
            Err(VfsError::NotFound(_))
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
            Err(VfsError::NotFound(_))
        ));
        assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
        // Renaming a directory into its own descendant is rejected.
        access.mkdir(&path("/d")?, false)?;
        assert!(matches!(
            access.rename(&path("/d")?, &path("/d/inner")?),
            Err(VfsError::InvalidPath(_))
        ));
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
        let mut access = backend.acquire(ExecId::vend())?;
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
                matches!(result, Err(VfsError::PermissionDenied(_))),
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
        let mut access = backend.acquire(ExecId::vend())?;
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
            Err(VfsError::NotADirectory(_))
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
            Err(VfsError::NotADirectory(_))
        ));
        assert!(matches!(
            HostBackend::rooted(temp.path().join("missing")),
            Err(VfsError::NotFound(_))
        ));
        Ok(())
    }
}
