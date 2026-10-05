use super::MemoryBackend;
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

/// Returns a session on a backend pre-populated through the write
/// path, so seeding exercises the same code the tests do.
fn seeded(files: &[(&str, &str)]) -> Result<Box<dyn VfsAccess>, VfsError> {
    let mut backend = MemoryBackend::new();
    let mut access = backend.acquire(&context())?;
    for (name, text) in files {
        access.write(&path(name)?, text.as_bytes())?;
    }
    Ok(access)
}

#[test]
fn read_returns_the_exact_bytes_stored() -> Result<(), VfsError> {
    let mut backend = MemoryBackend::new();
    let mut access = backend.acquire(&context())?;
    let bytes = [0x00_u8, 0xff, 0x00, 0x7f];
    access.write(&path("/bin.dat")?, &bytes)?;
    assert_eq!(access.read(&path("/bin.dat")?)?, bytes);
    Ok(())
}

#[test]
fn read_of_an_absent_path_is_not_found() -> Result<(), VfsError> {
    let access = seeded(&[])?;
    assert!(matches!(
        access.read(&path("/missing.txt")?),
        Err(VfsError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn read_of_a_directory_is_is_a_directory() -> Result<(), VfsError> {
    let access = seeded(&[("/dir/f.txt", "x")])?;
    assert!(matches!(
        access.read(&path("/dir")?),
        Err(VfsError::IsADirectory { .. })
    ));
    Ok(())
}

#[test]
fn read_range_slices_bytes_and_clips_at_the_end() -> Result<(), VfsError> {
    let access = seeded(&[("/f.txt", "hello world")])?;
    assert_eq!(access.read_range(&path("/f.txt")?, 6, 5)?, b"world");
    assert_eq!(access.read_range(&path("/f.txt")?, 6, 100)?, b"world");
    assert!(access.read_range(&path("/f.txt")?, 100, 5)?.is_empty());
    Ok(())
}

#[test]
fn write_creates_overwrites_and_materializes_ancestor_directories() -> Result<(), VfsError> {
    let mut access = seeded(&[])?;
    access.write(&path("/a/b/f.txt")?, b"one")?;
    assert_eq!(access.read(&path("/a/b/f.txt")?)?, b"one");
    // No mkdir was needed; the ancestors exist.
    assert!(access.exists(&path("/a")?)?);
    assert!(access.exists(&path("/a/b")?)?);
    access.write(&path("/a/b/f.txt")?, b"two")?;
    assert_eq!(access.read(&path("/a/b/f.txt")?)?, b"two");
    Ok(())
}

#[test]
fn write_at_a_directory_path_is_rejected_without_touching_the_tree() -> Result<(), VfsError> {
    let mut access = seeded(&[("/dir/f.txt", "x")])?;
    assert!(matches!(
        access.write(&path("/dir")?, b"y"),
        Err(VfsError::IsADirectory { .. })
    ));
    assert_eq!(access.read(&path("/dir/f.txt")?)?, b"x");
    Ok(())
}

#[test]
fn append_creates_when_absent_and_extends_when_present() -> Result<(), VfsError> {
    let mut access = seeded(&[])?;
    access.append(&path("/log.txt")?, b"first\n")?;
    access.append(&path("/log.txt")?, b"second")?;
    assert_eq!(access.read(&path("/log.txt")?)?, b"first\nsecond");
    Ok(())
}

#[test]
fn remove_of_an_absent_path_is_not_found() -> Result<(), VfsError> {
    let mut access = seeded(&[])?;
    assert!(matches!(
        access.remove(&path("/gone.txt")?, false),
        Err(VfsError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn remove_of_a_file_removes_it() -> Result<(), VfsError> {
    let mut access = seeded(&[("/f.txt", "x")])?;
    access.remove(&path("/f.txt")?, false)?;
    assert!(!access.exists(&path("/f.txt")?)?);
    Ok(())
}

#[test]
fn remove_of_a_nonempty_directory_without_recursive_is_an_error() -> Result<(), VfsError> {
    let mut access = seeded(&[("/dir/f.txt", "x")])?;
    assert!(matches!(
        access.remove(&path("/dir")?, false),
        Err(VfsError::DirectoryNotEmpty { .. })
    ));
    // The failed removal changed nothing.
    assert_eq!(access.read(&path("/dir/f.txt")?)?, b"x");
    assert!(access.exists(&path("/dir")?)?);
    Ok(())
}

#[test]
fn remove_of_an_empty_directory_without_recursive_succeeds() -> Result<(), VfsError> {
    let mut access = seeded(&[])?;
    access.mkdir(&path("/empty")?, false)?;
    access.remove(&path("/empty")?, false)?;
    assert!(!access.exists(&path("/empty")?)?);
    Ok(())
}

#[test]
fn remove_with_recursive_deletes_the_whole_subtree() -> Result<(), VfsError> {
    let mut access = seeded(&[("/d/a.txt", "a"), ("/d/sub/b.txt", "b"), ("/keep.txt", "k")])?;
    access.remove(&path("/d")?, true)?;
    assert!(!access.exists(&path("/d")?)?);
    assert!(!access.exists(&path("/d/sub")?)?);
    assert!(!access.exists(&path("/d/sub/b.txt")?)?);
    assert_eq!(access.read(&path("/keep.txt")?)?, b"k");
    Ok(())
}

#[test]
fn the_namespace_root_cannot_be_removed() -> Result<(), VfsError> {
    let mut access = seeded(&[("/f.txt", "x")])?;
    assert!(matches!(
        access.remove(&path("/")?, true),
        Err(VfsError::PermissionDenied { .. })
    ));
    assert_eq!(access.read(&path("/f.txt")?)?, b"x");
    Ok(())
}

#[test]
fn exists_distinguishes_files_directories_and_absence() -> Result<(), VfsError> {
    let access = seeded(&[("/dir/f.txt", "x")])?;
    assert!(access.exists(&path("/dir/f.txt")?)?);
    assert!(access.exists(&path("/dir")?)?);
    assert!(access.exists(&path("/")?)?);
    assert!(!access.exists(&path("/dir/missing.txt")?)?);
    Ok(())
}

#[test]
fn glob_matches_star_within_a_segment_and_double_star_across() -> Result<(), VfsError> {
    let access = seeded(&[
        ("/src/a.rs", ""),
        ("/src/b.rs", ""),
        ("/src/deep/c.rs", ""),
        ("/notes/today.md", ""),
    ])?;
    assert_eq!(
        access.glob("/src/*.rs")?,
        vec!["/src/a.rs".to_owned(), "/src/b.rs".to_owned()]
    );
    assert_eq!(
        access.glob("/src/**/*.rs")?,
        vec![
            "/src/a.rs".to_owned(),
            "/src/b.rs".to_owned(),
            "/src/deep/c.rs".to_owned(),
        ]
    );
    assert_eq!(access.glob("/**/*.md")?, vec!["/notes/today.md".to_owned()]);
    Ok(())
}

#[test]
fn glob_results_are_sorted_and_include_directories() -> Result<(), VfsError> {
    let access = seeded(&[("/d/b.txt", ""), ("/d/a.txt", ""), ("/d/sub/c.txt", "")])?;
    assert_eq!(
        access.glob("/d/*")?,
        vec![
            "/d/a.txt".to_owned(),
            "/d/b.txt".to_owned(),
            "/d/sub".to_owned(),
        ]
    );
    Ok(())
}

#[test]
fn glob_rejects_invalid_patterns() -> Result<(), VfsError> {
    let access = seeded(&[("/f.txt", "x")])?;
    assert_eq!(
        access.glob("/a/***/b"),
        Err(VfsError::InvalidPath {
            path: "/a/***/b".to_owned(),
            reason: PathReason::Wildcard,
        })
    );
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
fn list_returns_sorted_entries_with_stats() -> Result<(), VfsError> {
    let access = seeded(&[("/d/b.txt", "bb"), ("/d/a.txt", "a"), ("/d/sub/c.txt", "c")])?;
    let entries = access.list(&path("/d")?)?;
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, vec!["a.txt", "b.txt", "sub"]);
    assert_eq!(entries[0].stat.file_type, FileType::File);
    assert_eq!(entries[0].stat.size, 1);
    assert_eq!(entries[2].stat.file_type, FileType::Directory);
    assert!(entries.iter().all(|entry| entry.description.is_none()));
    Ok(())
}

#[test]
fn list_of_a_file_or_an_absent_path_is_an_error() -> Result<(), VfsError> {
    let access = seeded(&[("/f.txt", "x")])?;
    assert!(matches!(
        access.list(&path("/f.txt")?),
        Err(VfsError::NotADirectory { .. })
    ));
    assert!(matches!(
        access.list(&path("/missing")?),
        Err(VfsError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn stat_reports_kinds_and_sizes_without_fabricated_times() -> Result<(), VfsError> {
    let access = seeded(&[("/dir/f.txt", "hello")])?;
    let file = access.stat(&path("/dir/f.txt")?)?;
    assert_eq!(file.file_type, FileType::File);
    assert_eq!(file.size, 5);
    assert!(file.mode.is_none() && file.modified.is_none() && file.created.is_none());
    let dir = access.stat(&path("/dir")?)?;
    assert_eq!(dir.file_type, FileType::Directory);
    assert!(matches!(
        access.stat(&path("/missing")?),
        Err(VfsError::NotFound { .. })
    ));
    Ok(())
}

#[test]
fn mkdir_creates_directories_and_rejects_existing_paths() -> Result<(), VfsError> {
    let mut access = seeded(&[("/f.txt", "x")])?;
    access.mkdir(&path("/new")?, false)?;
    assert!(access.exists(&path("/new")?)?);
    assert!(matches!(
        access.mkdir(&path("/new")?, false),
        Err(VfsError::AlreadyExists { .. })
    ));
    assert!(matches!(
        access.mkdir(&path("/f.txt")?, false),
        Err(VfsError::AlreadyExists { .. })
    ));
    Ok(())
}

#[test]
fn mkdir_without_recursive_requires_an_existing_parent() -> Result<(), VfsError> {
    let mut access = seeded(&[])?;
    // The error names the target path, not a sentence about its
    // parent, matching the real-filesystem backend's own NotFound spelling.
    assert_eq!(
        access.mkdir(&path("/a/b")?, false),
        Err(VfsError::NotFound {
            path: "/a/b".to_owned(),
        })
    );
    access.mkdir(&path("/a/b")?, true)?;
    assert!(access.exists(&path("/a")?)?);
    assert!(access.exists(&path("/a/b")?)?);
    Ok(())
}

#[test]
fn mkdir_through_a_file_parent_is_not_a_directory() -> Result<(), VfsError> {
    let mut access = seeded(&[("/f.txt", "x")])?;
    assert!(matches!(
        access.mkdir(&path("/f.txt/g")?, true),
        Err(VfsError::NotADirectory { .. })
    ));
    Ok(())
}

#[test]
fn rename_moves_a_file_and_leaves_nothing_behind() -> Result<(), VfsError> {
    let mut access = seeded(&[("/from.txt", "data")])?;
    access.rename(&path("/from.txt")?, &path("/sub/to.txt")?)?;
    assert!(!access.exists(&path("/from.txt")?)?);
    assert_eq!(access.read(&path("/sub/to.txt")?)?, b"data");
    Ok(())
}

#[test]
fn rename_moves_a_directory_subtree() -> Result<(), VfsError> {
    let mut access = seeded(&[("/d/a.txt", "a"), ("/d/sub/b.txt", "b")])?;
    access.rename(&path("/d")?, &path("/moved")?)?;
    assert!(!access.exists(&path("/d")?)?);
    assert_eq!(access.read(&path("/moved/a.txt")?)?, b"a");
    assert_eq!(access.read(&path("/moved/sub/b.txt")?)?, b"b");
    Ok(())
}

#[test]
fn renaming_a_directory_onto_the_root_is_rejected() -> Result<(), VfsError> {
    let mut access = seeded(&[("/d/a.txt", "a"), ("/other.txt", "o")])?;
    assert!(matches!(
        access.rename(&path("/d")?, &path("/")?),
        Err(VfsError::PermissionDenied { .. })
    ));
    // The failed rename changed nothing: the subtree is intact.
    assert_eq!(access.read(&path("/d/a.txt")?)?, b"a");
    assert_eq!(access.read(&path("/other.txt")?)?, b"o");
    let root: Vec<String> = access
        .list(&path("/")?)?
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(root, vec!["d".to_owned(), "other.txt".to_owned()]);
    Ok(())
}

#[test]
fn a_failed_rename_leaves_source_and_destination_unchanged() -> Result<(), VfsError> {
    let mut access = seeded(&[("/dst.txt", "old")])?;
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
    Ok(())
}

#[test]
fn copy_duplicates_a_files_bytes() -> Result<(), VfsError> {
    let mut access = seeded(&[("/src.txt", "data")])?;
    access.copy(&path("/src.txt")?, &path("/dst.txt")?)?;
    assert_eq!(access.read(&path("/src.txt")?)?, b"data");
    assert_eq!(access.read(&path("/dst.txt")?)?, b"data");
    Ok(())
}

#[test]
fn copy_rejects_directories_and_a_failed_copy_changes_nothing() -> Result<(), VfsError> {
    let mut access = seeded(&[("/d/f.txt", "x"), ("/dst.txt", "old")])?;
    assert!(matches!(
        access.copy(&path("/d")?, &path("/dst.txt")?),
        Err(VfsError::IsADirectory { .. })
    ));
    assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
    assert!(matches!(
        access.copy(&path("/missing.txt")?, &path("/dst.txt")?),
        Err(VfsError::NotFound { .. })
    ));
    assert_eq!(access.read(&path("/dst.txt")?)?, b"old");
    Ok(())
}

#[test]
fn acquire_and_release_accept_attribution_as_a_no_op() -> Result<(), VfsError> {
    let mut backend = MemoryBackend::new();
    let mut first = backend.acquire(&context())?;
    first.write(&path("/f.txt")?, b"shared")?;
    // A second identity's session sees the same map.
    let second = backend.acquire(&context())?;
    assert_eq!(second.read(&path("/f.txt")?)?, b"shared");
    drop(first);
    drop(second);
    backend.release(ExecId::vend())?;
    Ok(())
}

#[test]
fn the_default_is_a_meaningful_empty_backend() -> Result<(), VfsError> {
    let mut backend = MemoryBackend::default();
    let access = backend.acquire(&context())?;
    assert!(access.exists(&path("/")?)?);
    assert!(!access.exists(&path("/anything")?)?);
    assert!(access.list(&path("/")?)?.is_empty());
    Ok(())
}
