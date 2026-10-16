//! Tests for the real-filesystem backend, split by topic. The temporary
//! directory and the rooted session helper live here because every
//! topic module uses them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::files::atomic_write;
use super::{RealBackend, Writability, map_io};
use crate::error::VfsError;
use crate::stat::FileType;
use crate::test_support::{context, path};
use crate::traits::{Vfs, VfsAccess};

mod atomicity;
mod links;
/// The rooted-path, idempotent-remove, and split-glob semantics of
/// the public capability, exercised over the real-filesystem backend.
mod semantics;

/// Tells apart roots one process makes; nextest runs each test in its own
/// process, so the process id tells those apart.
static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// A real-filesystem folder at
/// `<temp>/promptforge-tests/<crate>/<label>-<pid>-<n>`, removed on drop.
/// Its name reads no clock and no random generator.
#[derive(Debug)]
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    /// Makes the folder for the test named `label`, first removing anything
    /// an earlier process with the same id left at that path.
    #[track_caller]
    fn new(label: &str) -> Self {
        let n = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join("promptforge-tests")
            .join(env!("CARGO_PKG_NAME"))
            .join(format!("{label}-{}-{n}", std::process::id()));
        let cleared = match fs::symlink_metadata(&path) {
            Ok(found) if found.is_dir() => fs::remove_dir_all(&path),
            Ok(_) => fs::remove_file(&path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        };
        if let Err(err) = cleared.and_then(|()| fs::create_dir_all(&path)) {
            panic!("the temp root {} must be created: {err}", path.display());
        }
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_dir_all(&self.path)
            && !std::thread::panicking()
        {
            panic!(
                "the temp root {} must be removed: {err}",
                self.path.display()
            );
        }
    }
}

/// Acquires a session on a rooted backend over `dir`.
fn rooted_access(dir: &Path) -> Result<Box<dyn VfsAccess>, VfsError> {
    let mut backend = RealBackend::rooted(dir)?;
    backend.acquire(&context())
}

#[test]
fn a_rooted_backend_round_trips_files_and_directories() -> Result<(), VfsError> {
    let temp = TempRoot::new("a_rooted_backend_round_trips_files_and_directories");
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
    // The OS tracks modification times; honesty permits Some here.
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
    // The mounted root itself cannot be removed, renamed, or replaced.
    let root = || VfsError::RootProtected {
        path: "/".to_owned(),
    };
    assert_eq!(access.remove(&path("/")?, true), Err(root()));
    assert_eq!(access.rename(&path("/")?, &path("/b")?), Err(root()));
    assert_eq!(access.rename(&path("/a")?, &path("/")?), Err(root()));
    access.remove(&path("/a")?, true)?;
    assert!(!access.exists(&path("/a")?)?);
    Ok(())
}

#[test]
fn a_read_only_backend_rejects_every_mutation_but_serves_reads() -> Result<(), VfsError> {
    let temp = TempRoot::new("a_read_only_backend_rejects_every_mutation_but_serves_reads");
    fs::write(temp.path().join("keep.txt"), b"keep")
        .map_err(|err| map_io("seeding the kept file", &err))?;
    let mut backend = RealBackend::rooted(temp.path())?.with_writability(Writability::ReadOnly);
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
            matches!(result, Err(VfsError::ReadOnly { .. })),
            "expected a read-only refusal, got {result:?}"
        );
    }
    assert_eq!(access.read(&path("/keep.txt")?)?, b"keep");
    assert!(!temp.path().join("new.txt").exists());
    Ok(())
}

#[test]
fn an_identity_backend_maps_virtual_paths_directly_to_real_paths() -> Result<(), VfsError> {
    let temp = TempRoot::new("an_identity_backend_maps_virtual_paths_directly_to_real_paths");
    let real_file = temp.path().join("identity.txt");
    let virtual_spelling = identity_spelling(&real_file);
    let mut backend = RealBackend::identity();
    let mut access = backend.acquire(&context())?;
    access.write(&path(&virtual_spelling)?, b"direct")?;
    assert_eq!(
        fs::read(&real_file).map_err(|err| map_io("reading the real file", &err))?,
        b"direct"
    );
    assert_eq!(access.read(&path(&virtual_spelling)?)?, b"direct");
    assert!(access.exists(&path(&virtual_spelling)?)?);
    Ok(())
}

#[test]
fn a_literal_glob_pattern_matches_the_file_itself() -> Result<(), VfsError> {
    let temp = TempRoot::new("a_literal_glob_pattern_matches_the_file_itself");
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
    let temp = TempRoot::new("exists_surfaces_backend_failures_as_errors");
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
    let temp = TempRoot::new("the_rooted_constructor_requires_an_existing_directory");
    fs::write(temp.path().join("f.txt"), b"x").map_err(|err| map_io("seeding the file", &err))?;
    assert!(matches!(
        RealBackend::rooted(temp.path().join("f.txt")),
        Err(VfsError::NotADirectory { .. })
    ));
    assert!(matches!(
        RealBackend::rooted(temp.path().join("missing")),
        Err(VfsError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn map_io_reports_the_canonical_path_not_the_os_sentence() {
    use std::io::ErrorKind;
    // The kind carries the OS failure; the `path` field holds the
    // canonical path alone, as the field's documented contract
    // promises, so a caller reading it gets a path, never OS text.
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
    // Any other failure, a denied permission included, keeps its kind,
    // its OS error code, and the OS text beside the path.
    let access_denied = if cfg!(windows) { 5 } else { 13 };
    match map_io("/x", &std::io::Error::from_raw_os_error(access_denied)) {
        VfsError::Io {
            path,
            kind,
            raw_os_error,
            message,
        } => {
            assert_eq!(path, "/x");
            assert_eq!(kind, ErrorKind::PermissionDenied);
            assert_eq!(raw_os_error, Some(access_denied));
            assert!(!message.is_empty());
        }
        other => panic!("expected an I/O failure, got {other:?}"),
    }
    match map_io("/x", &std::io::Error::from(ErrorKind::TimedOut)) {
        VfsError::Io {
            kind, raw_os_error, ..
        } => assert_eq!((kind, raw_os_error), (ErrorKind::TimedOut, None)),
        other => panic!("expected an I/O failure, got {other:?}"),
    }
}

/// The identity-mode virtual spelling of a real path: forward slashes,
/// and a leading slash before a Windows drive letter.
fn identity_spelling(real: &Path) -> String {
    let spelled = real
        .to_str()
        .expect("the temp directory's path is UTF-8")
        .replace('\\', "/");
    if spelled.as_bytes().get(1) == Some(&b':') {
        format!("/{spelled}")
    } else {
        spelled
    }
}

/// A real file name that is not UTF-8: `b`, an unpaired surrogate, `d`.
#[cfg(windows)]
fn non_utf8_name() -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    std::ffi::OsString::from_wide(&[u16::from(b'b'), 0xd800, u16::from(b'd')])
}

/// A real file name that is not UTF-8: `b`, a lone continuation byte, `d`.
#[cfg(unix)]
fn non_utf8_name() -> std::ffi::OsString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(b"b\x80d").to_owned()
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS file systems refuse names that are not UTF-8"
)]
fn a_real_name_that_is_not_utf8_is_refused_with_its_offset() -> Result<(), VfsError> {
    let temp = TempRoot::new("a_real_name_that_is_not_utf8_is_refused_with_its_offset");
    let dir = temp.path().join("dir");
    fs::create_dir(&dir).map_err(|err| map_io("creating the directory", &err))?;
    fs::write(dir.join(non_utf8_name()), b"x")
        .map_err(|err| map_io("seeding the non-UTF-8 name", &err))?;
    let access = rooted_access(temp.path())?;
    let refused = || VfsError::NotUtf8 {
        path: "/dir".to_owned(),
        valid_up_to: 1,
    };
    assert_eq!(access.list(&path("/dir")?), Err(refused()));
    assert_eq!(access.glob("/dir/*"), Err(refused()));
    assert_eq!(access.glob("/**"), Err(refused()));
    let named = dir.join(non_utf8_name());
    fs::remove_file(&named).map_err(|err| map_io("removing the name", &err))?;
    fs::create_dir(&named).map_err(|err| map_io("creating the directory", &err))?;
    // A root that is not UTF-8 is refused with the part that is.
    let utf8_part = format!("{}/b", identity_spelling(&dir).trim_start_matches('/'));
    match RealBackend::rooted(&named) {
        Err(VfsError::NotUtf8 { path, valid_up_to }) => {
            assert!(path.replace('\\', "/").ends_with(&utf8_part), "{path}");
            assert_eq!(valid_up_to, path.len());
        }
        other => panic!("a root that is not UTF-8 must be refused, got {other:?}"),
    }
    Ok(())
}

#[test]
fn temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder() {
    let label = "temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder";
    let (first, second) = (TempRoot::new(label), TempRoot::new(label));
    assert_ne!(first.path(), second.path());
    let dropped = first.path().to_owned();
    fs::write(dropped.join("f.txt"), b"x").expect("the first root takes a file");
    drop(first);
    assert!(!dropped.exists(), "{} survived its drop", dropped.display());
    assert!(second.path().is_dir(), "the second root outlives the first");
}

#[test]
fn an_atomic_write_addressing_only_a_root_is_a_backend_failure() {
    // A destination without a parent or a file name names nothing
    // below the root; it is a backend impossibility, not a path rule
    // a caller broke, so it must not render as "path is empty".
    let err = atomic_write(
        Path::new("/"),
        &path("/").expect("the root is canonical"),
        b"x",
    )
    .unwrap_err();
    assert!(
        matches!(err, VfsError::Backend { .. }),
        "a root-only destination is not a path-validation failure: {err:?}"
    );
}
