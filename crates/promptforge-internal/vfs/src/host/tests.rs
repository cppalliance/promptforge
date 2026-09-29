//! Tests for the host backend, split by topic. The temporary
//! directory and the rooted session helper live here because every
//! topic module uses them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::files::atomic_write;
use super::resolve::identity_to_virtual;
use super::{HostBackend, map_io};
use crate::error::VfsError;
use crate::handle::Scope;
use crate::path::{VfsPath, canonicalize_absolute};
use crate::stat::FileType;
use crate::traits::{AcquireContext, ExecId, Vfs, VfsAccess};

mod atomicity;
mod links;
/// The rooted-path, idempotent-remove, and split-glob semantics of
/// the public capability, exercised over the host backend.
mod semantics;

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
    fs::write(temp.path().join("f.txt"), b"x").map_err(|err| map_io("seeding the file", &err))?;
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
