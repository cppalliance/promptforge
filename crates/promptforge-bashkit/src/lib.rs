//! Spike: the Bashkit engine's `FsBackend` trait implemented over the
//! shared VFS handle.
//!
//! The deliverable is evidence that `Vfs` subsumes Bashkit's storage
//! contract: whole-file reads serve from `read`, `symlink`/`chmod` return
//! the engine's unsupported error, the first four file types map directly
//! and the three specials map to `File` with a trace, and an absent `Stat`
//! mode emits the 0o644/0o755 defaults. The adapter captures the current
//! [`ExecId`] at exec start: one identity per engine session, so the
//! claims model attributes every script operation to that session.

use std::io::{Error as IoError, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bashkit::{DirEntry, Error, FileType as BashFileType, FsBackend, Metadata, Result};
use shared_vfs::{Access, ExecId, FileType as VfsFileType, Stat, VfsError, VfsRef};

/// A Bashkit storage backend serving from a VFS handle.
///
/// Constructing one acquires an [`Access`] capability: the adapter holds
/// one [`ExecId`] for the engine session's lifetime, so every script
/// operation is attributed to that identity and the claims model sees
/// the session as one thread of execution. The engine's `PosixFs`
/// wrapper enforces POSIX semantics above this raw storage layer.
#[derive(Debug)]
pub struct VfsBackend {
    access: Access,
}

impl VfsBackend {
    /// Captures a fresh identity from `vfs`: call at exec start.
    #[must_use]
    pub fn new(vfs: &VfsRef) -> VfsBackend {
        VfsBackend {
            access: vfs.acquire(),
        }
    }

    /// Binds the backend to an existing capability: a host that already
    /// holds the run's [`Access`] keeps the script under that identity.
    #[must_use]
    pub fn from_access(access: Access) -> VfsBackend {
        VfsBackend { access }
    }

    /// The identity this backend's operations are attributed to.
    #[must_use]
    pub fn id(&self) -> ExecId {
        self.access.id()
    }
}

/// The engine hands `Path` values; the virtual namespace is POSIX-shaped
/// text. Lossy conversion with separator normalization is sufficient:
/// the engine never produces host-native paths here.
fn vfs_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Maps the VFS error onto the engine's io-error channel, preserving the
/// kind so builtins report the right failure (`PermissionDenied` for a
/// claims conflict or policy denial, `Unsupported` for unimplemented
/// operations, and so on).
fn to_io(error: VfsError) -> Error {
    let kind = match &error {
        VfsError::NotFound(_) => ErrorKind::NotFound,
        VfsError::PermissionDenied(_) | VfsError::Conflict(_) => ErrorKind::PermissionDenied,
        VfsError::AlreadyExists(_) => ErrorKind::AlreadyExists,
        VfsError::InvalidPath(_) => ErrorKind::InvalidInput,
        VfsError::NotADirectory(_) => ErrorKind::NotADirectory,
        VfsError::IsADirectory(_) => ErrorKind::IsADirectory,
        VfsError::DirectoryNotEmpty(_) => ErrorKind::DirectoryNotEmpty,
        VfsError::Unsupported(_) => ErrorKind::Unsupported,
        _ => ErrorKind::Other,
    };
    IoError::new(kind, error).into()
}

/// The first four kinds map directly; the three specials map to `File`
/// with a trace (unreachable in practice: neither our v1 backends nor
/// the engine's ever produce them).
fn file_type(kind: VfsFileType) -> BashFileType {
    match kind {
        VfsFileType::File => BashFileType::File,
        VfsFileType::Directory => BashFileType::Directory,
        VfsFileType::Symlink => BashFileType::Symlink,
        VfsFileType::Fifo => BashFileType::Fifo,
        special => {
            tracing::warn!(
                ?special,
                "VFS special file type reported to the engine as File"
            );
            BashFileType::File
        }
    }
}

/// The VFS says `None` rather than fabricating; the engine's `Metadata`
/// has no options, so an absent mode emits the 0o644/0o755 defaults and
/// absent timestamps become the epoch - deterministic, never an invented
/// `now()`.
fn metadata(stat: &Stat) -> Metadata {
    let mode = stat.mode.unwrap_or(match stat.file_type {
        VfsFileType::Directory => 0o755,
        _ => 0o644,
    });
    Metadata {
        file_type: file_type(stat.file_type),
        size: stat.size,
        mode,
        modified: stat.modified.unwrap_or(SystemTime::UNIX_EPOCH),
        created: stat.created.unwrap_or(SystemTime::UNIX_EPOCH),
    }
}

/// The engine's unsupported error, matching its own convention of an
/// io error with `ErrorKind::Unsupported`.
fn unsupported(op: &str) -> Error {
    IoError::new(
        ErrorKind::Unsupported,
        format!("{op} is not supported by the VFS adapter"),
    )
    .into()
}

#[bashkit::async_trait]
impl FsBackend for VfsBackend {
    async fn read(&self, path: &Path) -> Result<Vec<u8>> {
        self.access.read(&vfs_path(path)).map_err(to_io)
    }

    async fn write(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.access.write(&vfs_path(path), content).map_err(to_io)
    }

    async fn append(&self, path: &Path, content: &[u8]) -> Result<()> {
        self.access.append(&vfs_path(path), content).map_err(to_io)
    }

    async fn mkdir(&self, path: &Path, recursive: bool) -> Result<()> {
        self.access.mkdir(&vfs_path(path), recursive).map_err(to_io)
    }

    async fn remove(&self, path: &Path, recursive: bool) -> Result<()> {
        self.access
            .remove(&vfs_path(path), recursive)
            .map_err(to_io)
    }

    async fn stat(&self, path: &Path) -> Result<Metadata> {
        self.access
            .stat(&vfs_path(path))
            .map(|stat| metadata(&stat))
            .map_err(to_io)
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>> {
        let entries = self.access.list(&vfs_path(path)).map_err(to_io)?;
        Ok(entries
            .into_iter()
            .map(|entry| DirEntry {
                name: entry.name,
                metadata: metadata(&entry.stat),
            })
            .collect())
    }

    async fn exists(&self, path: &Path) -> Result<bool> {
        self.access.exists(&vfs_path(path)).map_err(to_io)
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.access
            .rename(&vfs_path(from), &vfs_path(to))
            .map_err(to_io)
    }

    async fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        self.access
            .copy(&vfs_path(from), &vfs_path(to))
            .map_err(to_io)
    }

    async fn symlink(&self, _target: &Path, _link: &Path) -> Result<()> {
        Err(unsupported("symlink"))
    }

    async fn read_link(&self, _path: &Path) -> Result<PathBuf> {
        Err(unsupported("read_link"))
    }

    async fn chmod(&self, _path: &Path, _mode: u32) -> Result<()> {
        Err(unsupported("chmod"))
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::path::Path;
    use std::sync::Arc;

    use bashkit::{Bash, Error, FsBackend, PosixFs};
    use shared_vfs::{FileType as VfsFileType, MemoryBackend, VfsRef};

    use super::{VfsBackend, file_type};
    use bashkit::FileType as BashFileType;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// An engine whose entire filesystem is the VFS handle, with POSIX
    /// semantics enforced by the engine's own wrapper.
    fn engine(vfs: &VfsRef) -> Bash {
        let backend = VfsBackend::new(vfs);
        let fs = Arc::new(PosixFs::new(backend));
        Bash::builder().fs(fs).build()
    }

    #[tokio::test]
    async fn an_ls_cat_grep_script_runs_against_a_mounted_memory_backend() -> TestResult {
        let vfs = VfsRef::builder().mount("/", MemoryBackend::new()).build();
        let mut bash = engine(&vfs);
        let result = bash
            .exec(
                "mkdir -p /tmp/docs && echo hello > /tmp/docs/a.txt \
                 && ls /tmp/docs && cat /tmp/docs/a.txt \
                 && grep hello /tmp/docs/a.txt",
            )
            .await?;
        assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
        let stdout = result.stdout.text_lossy().into_owned();
        assert!(stdout.contains("a.txt"), "ls lists the file: {stdout}");
        assert!(
            stdout.contains("hello"),
            "cat and grep serve reads: {stdout}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_ls_cat_grep_script_runs_against_the_store_mount() -> TestResult {
        let vfs = promptforge_vfs::empty();
        vfs.acquire()
            .write("/_promptforge/store/paper.md", b"# Draft\nhello world\n")?;
        let mut bash = engine(&vfs);
        let result = bash
            .exec(
                "ls /_promptforge/store && cat /_promptforge/store/paper.md \
                 && grep hello /_promptforge/store/paper.md",
            )
            .await?;
        assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
        let stdout = result.stdout.text_lossy().into_owned();
        assert!(stdout.contains("paper.md"), "ls lists the store: {stdout}");
        assert!(
            stdout.contains("hello world"),
            "cat and grep read the store: {stdout}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn symlink_and_chmod_return_the_engine_unsupported_error() -> TestResult {
        let vfs = VfsRef::new(MemoryBackend::new());
        let backend = VfsBackend::new(&vfs);
        match backend.symlink(Path::new("/a"), Path::new("/b")).await {
            Err(Error::Io(io)) => assert_eq!(io.kind(), ErrorKind::Unsupported),
            other => panic!("expected an unsupported io error, got {other:?}"),
        }
        match backend.read_link(Path::new("/a")).await {
            Err(Error::Io(io)) => assert_eq!(io.kind(), ErrorKind::Unsupported),
            other => panic!("expected an unsupported io error, got {other:?}"),
        }
        match backend.chmod(Path::new("/a"), 0o600).await {
            Err(Error::Io(io)) => assert_eq!(io.kind(), ErrorKind::Unsupported),
            other => panic!("expected an unsupported io error, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn an_absent_stat_mode_emits_the_posix_defaults() -> TestResult {
        let vfs = VfsRef::new(MemoryBackend::new());
        let backend = VfsBackend::new(&vfs);
        backend.write(Path::new("/f.txt"), b"x").await?;
        backend.mkdir(Path::new("/d"), false).await?;
        // The memory backend honestly reports mode None; the adapter
        // emits the engine's expected defaults instead.
        assert_eq!(backend.stat(Path::new("/f.txt")).await?.mode, 0o644);
        assert_eq!(backend.stat(Path::new("/d")).await?.mode, 0o755);
        Ok(())
    }

    #[test]
    fn special_file_types_map_to_file_and_the_first_four_map_directly() {
        assert_eq!(file_type(VfsFileType::File), BashFileType::File);
        assert_eq!(file_type(VfsFileType::Directory), BashFileType::Directory);
        assert_eq!(file_type(VfsFileType::Symlink), BashFileType::Symlink);
        assert_eq!(file_type(VfsFileType::Fifo), BashFileType::Fifo);
        assert_eq!(file_type(VfsFileType::Socket), BashFileType::File);
        assert_eq!(file_type(VfsFileType::CharDevice), BashFileType::File);
        assert_eq!(file_type(VfsFileType::BlockDevice), BashFileType::File);
    }

    #[test]
    fn each_adapter_captures_a_fresh_exec_identity_at_exec_start() {
        let vfs = VfsRef::new(MemoryBackend::new());
        let first = VfsBackend::new(&vfs);
        let second = VfsBackend::new(&vfs);
        assert_ne!(first.id(), second.id());
        // A host that already holds a capability binds it explicitly.
        let access = vfs.acquire();
        let bound = VfsBackend::from_access(access);
        assert_ne!(bound.id(), first.id());
    }
}
