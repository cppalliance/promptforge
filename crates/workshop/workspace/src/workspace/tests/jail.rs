//! Jail edge cases: the path-spelling tricks that must never escape the
//! grants, how linked folders and files list and open, and the CI-aware
//! helper that turns a silent symlink skip into a CI failure.
//!
//! The confinement pipeline in `workspace/confine.rs` rejects `..` and
//! Windows alternate-data-stream colons lexically, canonicalizes the rest
//! (resolving symlinks, junctions, case, and verbatim `\\?\` prefixes), and
//! prefix-matches the canonical path against the canonical grants. These
//! tests pin that behavior for the spellings a request can arrive in.

use super::*;

use crate::workspace::confine::modified_ms;

/// Turns a silent skip into a failure under CI, and prints the reason
/// otherwise so the caller can `return`. The `ci` flag is read by the
/// caller through `std::env::var_os("CI").is_some()`, so no test ever calls
/// `std::env::set_var`, which is `unsafe` in Rust 2024 and forbidden here.
pub(super) fn symlink_unavailable(ci: bool, reason: &str) {
    assert!(!ci, "{reason}");
    eprintln!("skipping: {reason}");
}

#[test]
#[should_panic(expected = "symlink creation failed")]
fn the_ci_flag_turns_a_skip_into_a_failure() {
    symlink_unavailable(true, "symlink creation failed");
}

#[test]
fn without_ci_a_skip_prints_and_returns() {
    symlink_unavailable(false, "symlink creation failed");
}

/// Links `link` to the directory `target`: a symlink on Unix, a junction
/// on Windows, which needs no symlink privilege. Returns whether the link
/// was made.
fn link_dir(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        let outcome = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output();
        matches!(&outcome, Ok(output) if output.status.success())
    }
}

/// Sets the directory `dir`'s modified time to `time`. A Unix directory
/// never opens for writing; a Windows directory opens only with
/// `FILE_FLAG_BACKUP_SEMANTICS`, and setting its time needs
/// `FILE_WRITE_ATTRIBUTES`.
fn age_dir(dir: &Path, time: std::time::SystemTime) -> std::io::Result<()> {
    #[cfg(unix)]
    let file = fs::File::open(dir)?;
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        fs::File::options()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)?
    };
    file.set_modified(time)
}

/// The entry named `name` in `listing`.
fn listed<'a>(listing: &'a TreeListing, name: &str) -> &'a TreeEntry {
    listing
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed"))
}

#[test]
fn a_linked_folder_inside_a_grant_lists_as_a_directory_and_opens() {
    let (workspace, dir) = granted_dir();
    let target = dir.path().join("real");
    fs::create_dir(&target).expect("create the link target");
    fs::write(target.join("inside.txt"), "inside").expect("seed the link target");
    let link = dir.path().join("linked");
    if !link_dir(&target, &link) {
        symlink_unavailable(
            std::env::var_os("CI").is_some(),
            "directory link creation failed",
        );
        return;
    }
    let listing = workspace.tree(Some(dir.path())).expect("list the grant");
    let entry = listed(&listing, "linked");
    assert_eq!(entry.kind, EntryKind::Directory);
    assert_eq!(entry.size, 0);
    let opened = workspace
        .tree(Some(&link))
        .expect("a link to a granted folder opens");
    let names: Vec<&str> = opened
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["inside.txt"]);
}

#[test]
fn a_linked_folder_pointing_outside_the_grant_lists_as_a_directory_but_never_opens() {
    let (workspace, dir) = granted_dir();
    let outside = tempfile::TempDir::new().expect("outside tempdir");
    age_dir(
        outside.path(),
        std::time::UNIX_EPOCH + std::time::Duration::from_hours(24),
    )
    .expect("age the link target");
    let link = dir.path().join("escape");
    if !link_dir(outside.path(), &link) {
        symlink_unavailable(
            std::env::var_os("CI").is_some(),
            "directory link creation failed",
        );
        return;
    }
    let listing = workspace.tree(Some(dir.path())).expect("list the grant");
    let entry = listed(&listing, "escape");
    assert_eq!(entry.kind, EntryKind::Directory);
    assert_eq!(entry.size, 0);
    let own = fs::symlink_metadata(&link).expect("inspect the link itself");
    assert_eq!(entry.modified_ms, modified_ms(&own));
    assert_ne!(entry.modified_ms, 86_400_000, "the target's mtime leaked");
    let error = workspace
        .tree(Some(&link))
        .expect_err("opening a link out of the grant must be rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}

#[test]
fn a_dangling_link_lists_as_its_own_entry() {
    let (workspace, dir) = granted_dir();
    let target = dir.path().join("gone");
    fs::create_dir(&target).expect("create the link target");
    let link = dir.path().join("dangling");
    if !link_dir(&target, &link) {
        symlink_unavailable(
            std::env::var_os("CI").is_some(),
            "directory link creation failed",
        );
        return;
    }
    fs::remove_dir(&target).expect("remove the link target");
    let listing = workspace
        .tree(Some(dir.path()))
        .expect("a dangling link must not fail the listing");
    assert_eq!(listing.entries.len(), 1);
    let entry = listed(&listing, "dangling");
    #[cfg(windows)]
    assert_eq!(entry.kind, EntryKind::Directory);
    #[cfg(unix)]
    assert_eq!(entry.kind, EntryKind::File);
    assert_eq!(entry.size, 0);
    assert!(
        entry.exists,
        "an enumerated entry is on disk by construction"
    );
}

/// Links `link` to the file `target`. A Windows file symlink needs the
/// symlink privilege or Developer Mode. Returns whether the link was made.
fn link_file(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }
}

#[test]
fn a_linked_file_pointing_outside_the_grant_lists_by_its_own_metadata() {
    let (workspace, dir) = granted_dir();
    let outside = tempfile::TempDir::new().expect("outside tempdir");
    let target = outside.path().join("secret.txt");
    fs::write(&target, "secret").expect("seed the link target");
    fs::File::options()
        .write(true)
        .open(&target)
        .and_then(|file| {
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_hours(24))
        })
        .expect("age the link target");
    let link = dir.path().join("linked.txt");
    if !link_file(&target, &link) {
        symlink_unavailable(
            std::env::var_os("CI").is_some(),
            "file link creation failed",
        );
        return;
    }
    let listing = workspace.tree(Some(dir.path())).expect("list the grant");
    let entry = listed(&listing, "linked.txt");
    assert_eq!(entry.kind, EntryKind::File);
    assert_eq!(entry.size, 0, "a file link never reports its target's size");
    let own = fs::symlink_metadata(&link).expect("inspect the link itself");
    assert_eq!(entry.modified_ms, modified_ms(&own));
    assert_ne!(entry.modified_ms, 86_400_000, "the target's mtime leaked");
}

#[cfg(windows)]
fn verbatim(path: &Path) -> PathBuf {
    // `\\?\` is the Win32 verbatim (extended-length) prefix: the same file,
    // spelled a different way. Build it from the simplified DOS form so the
    // test always exercises the prefix; `canonicalize_simplified` strips it.
    PathBuf::from(format!("\\\\?\\{}", simplified(path).display()))
}

#[cfg(windows)]
fn unc(path: &Path) -> PathBuf {
    admin_share(&simplified(path))
}

#[cfg(windows)]
fn admin_share(path: &Path) -> PathBuf {
    // `\\localhost\C$\...` is the administrative-share spelling of a local
    // path; it canonicalizes to a UNC form that never matches a local grant.
    // A path without a drive has no such spelling, and a mangled one would
    // pass the jail tests by failing to resolve, so it panics instead.
    use std::path::{Component, Prefix};
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => Some(char::from(drive)),
            _ => None,
        },
        _ => None,
    };
    let Some(drive) = drive else {
        panic!("{} has no local drive to respell", path.display());
    };
    let mut respelled = PathBuf::from(format!(r"\\localhost\{drive}$"));
    respelled.extend(components);
    respelled
}

#[cfg(windows)]
#[test]
fn the_unc_respelling_takes_the_drive_from_the_path_prefix() {
    assert_eq!(
        admin_share(Path::new(r"C:\Temp\a.txt")),
        PathBuf::from(r"\\localhost\C$\Temp\a.txt")
    );
    assert_eq!(
        admin_share(Path::new(r"\\?\D:\Temp\a.txt")),
        PathBuf::from(r"\\localhost\D$\Temp\a.txt")
    );
}

#[cfg(windows)]
#[test]
#[should_panic(expected = "has no local drive")]
fn the_unc_respelling_refuses_a_path_without_a_local_drive() {
    admin_share(Path::new(r"\\server\share\a.txt"));
}

#[cfg(windows)]
#[test]
fn a_verbatim_spelling_of_a_granted_path_is_admitted() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    fs::write(&file, "hello").expect("seed the granted file");
    let read = workspace
        .read_file(&verbatim(&file))
        .expect("a verbatim spelling of a granted path reads");
    assert_eq!(read.text, "hello");
}

#[cfg(windows)]
#[test]
fn a_verbatim_spelling_of_an_ungranted_path_is_rejected() {
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    fs::write(dir.path().join("a.txt"), "a").expect("seed the ungranted file");
    let error = workspace
        .read_file(&verbatim(&dir.path().join("a.txt")))
        .expect_err("a verbatim spelling of an ungranted path is rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}

#[cfg(windows)]
#[test]
fn a_unc_spelling_never_escapes_the_grants() {
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    fs::write(dir.path().join("a.txt"), "a").expect("seed the local file");
    // The UNC spelling either canonicalizes to a form that no local grant
    // prefix-matches (OutsideGrants) or fails to resolve on a host without
    // the administrative share (NotFound or ResolvePath). It must never
    // admit the path.
    let error = workspace
        .read_file(&unc(&dir.path().join("a.txt")))
        .expect_err("a UNC spelling must never be admitted");
    assert!(
        matches!(
            error,
            WorkspaceError::OutsideGrants
                | WorkspaceError::NotFound
                | WorkspaceError::ResolvePath { .. }
        ),
        "expected a rejection, got {error:?}"
    );
}

#[cfg(windows)]
#[test]
fn a_unc_spelling_of_a_granted_path_is_rejected() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    fs::write(&file, "hello").expect("seed the granted file");
    // A UNC spelling canonicalizes to a UNC form that never prefix-matches a
    // local grant, so even a granted path is refused (OutsideGrants); on a
    // host without the administrative share the resolution fails instead
    // (NotFound or ResolvePath). It is never admitted.
    let error = workspace
        .read_file(&unc(&file))
        .expect_err("a UNC spelling of a granted path is never admitted");
    assert!(
        matches!(
            error,
            WorkspaceError::OutsideGrants
                | WorkspaceError::NotFound
                | WorkspaceError::ResolvePath { .. }
        ),
        "expected a rejection, got {error:?}"
    );
}

#[cfg(windows)]
#[test]
fn a_case_only_respelling_of_a_granted_root_is_admitted() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    fs::write(&file, "hello").expect("seed the granted file");
    let respelled = PathBuf::from(file.to_string_lossy().to_ascii_uppercase());
    let read = workspace
        .read_file(&respelled)
        .expect("a case-only respelling of a granted path reads");
    assert_eq!(read.text, "hello");
}

#[cfg(windows)]
#[test]
fn a_junction_inside_a_grant_pointing_outside_is_rejected() {
    let (workspace, dir) = granted_dir();
    let outside = tempfile::TempDir::new().expect("outside tempdir");
    fs::write(outside.path().join("secret.txt"), "secret").expect("seed the secret");
    let junction = dir.path().join("junction");
    if !link_dir(outside.path(), &junction) {
        symlink_unavailable(std::env::var_os("CI").is_some(), "junction creation failed");
        return;
    }
    let error = workspace
        .read_file(&junction.join("secret.txt"))
        .expect_err("a junction escape must be rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}
