use super::*;

/// A workspace with one granted tempdir, returned alongside so the
/// directory outlives the test.
fn granted_dir() -> (Workspace, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace.grant(dir.path()).expect("grant the tempdir");
    (workspace, dir)
}

/// The canonical, verbatim-prefix-free form grants are stored in.
fn simplified(path: &Path) -> PathBuf {
    canonicalize_simplified(path).expect("canonical")
}

#[test]
fn a_folder_grant_grants_the_folder_itself() {
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let granted = workspace.grant(dir.path()).expect("grant succeeds");
    assert_eq!(granted, simplified(dir.path()));
    assert_eq!(workspace.granted_roots(), vec![granted]);
}

#[test]
fn a_file_grant_grants_the_parent_directory() {
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = dir.path().join("dropped.txt");
    fs::write(&file, "x").expect("seed the dropped file");
    let granted = workspace.grant(&file).expect("grant succeeds");
    assert_eq!(granted, simplified(dir.path()));
    assert_eq!(workspace.granted_roots(), vec![granted]);
}

#[test]
fn files_read_and_write_inside_a_grant() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    let written = workspace
        .write_file(&file, "hello", None)
        .expect("write inside the grant");
    assert_eq!(written.text, "hello");
    assert_eq!(written.size, 5);
    let read = workspace.read_file(&file).expect("read inside the grant");
    assert_eq!(read.text, "hello");
    assert_eq!(read.size, 5);
    assert_eq!(read.token, written.token);
}

#[test]
fn writes_leave_no_temp_file_behind() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    let written = workspace
        .write_file(&file, "one", None)
        .expect("the create write succeeds");
    workspace
        .write_file(&file, "two", Some(&written.token))
        .expect("the overwrite succeeds");
    let names: Vec<String> = fs::read_dir(dir.path())
        .expect("the granted directory is listable")
        .map(|entry| {
            entry
                .expect("the entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        ["notes.txt"],
        "the atomic write's temp file must not survive the write"
    );
}

#[test]
fn paths_outside_every_grant_are_rejected() {
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    fs::write(dir.path().join("a.txt"), "a").expect("seed outside the grants");
    let error = workspace
        .read_file(&dir.path().join("a.txt"))
        .expect_err("an ungranted path must be rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}

#[test]
fn parent_components_are_rejected() {
    let (workspace, dir) = granted_dir();
    let escape = dir.path().join("..").join("anything.txt");
    let error = workspace
        .read_file(&escape)
        .expect_err("a .. component must be rejected");
    assert!(
        matches!(error, WorkspaceError::ForbiddenComponent),
        "expected ForbiddenComponent, got {error:?}"
    );
}

#[cfg(windows)]
#[test]
fn alternate_data_stream_names_are_rejected() {
    let (workspace, dir) = granted_dir();
    let stream = dir.path().join("notes.txt:secret");
    let error = workspace
        .write_file(&stream, "hidden", None)
        .expect_err("an alternate data stream name must be rejected");
    assert!(
        matches!(error, WorkspaceError::ForbiddenComponent),
        "expected ForbiddenComponent, got {error:?}"
    );
}

#[test]
fn a_symlink_escape_is_rejected() {
    let (workspace, dir) = granted_dir();
    let outside = tempfile::TempDir::new().expect("outside tempdir");
    fs::write(outside.path().join("secret.txt"), "secret").expect("seed the secret");
    let link = dir.path().join("link");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(outside.path(), &link);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(outside.path(), &link);
    let Ok(()) = linked else {
        // Symlink creation needs a privilege some Windows hosts lack.
        eprintln!("skipping: symlink creation failed");
        return;
    };
    let error = workspace
        .read_file(&link.join("secret.txt"))
        .expect_err("a symlink escape must be rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}

#[test]
fn a_dangling_symlink_write_is_rejected() {
    let (workspace, dir) = granted_dir();
    let outside = tempfile::TempDir::new().expect("outside tempdir");
    let target = outside.path().join("new.txt");
    let link = dir.path().join("link.txt");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&target, &link);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&target, &link);
    let Ok(()) = linked else {
        // Symlink creation needs a privilege some Windows hosts lack.
        eprintln!("skipping: symlink creation failed");
        return;
    };
    let error = workspace
        .write_file(&link, "payload", None)
        .expect_err("a write through a dangling symlink must be rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
    assert!(!target.exists(), "nothing may be written outside the grant");
}

#[test]
fn binary_files_are_rejected() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("bin.dat");
    fs::write(&file, [0x66, 0x00, 0x66]).expect("seed a binary file");
    let error = workspace
        .read_file(&file)
        .expect_err("a binary file must be rejected");
    assert!(
        matches!(error, WorkspaceError::BinaryFile),
        "expected BinaryFile, got {error:?}"
    );
}

#[test]
fn oversized_files_are_rejected() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("big.txt");
    let big = vec![b'x'; usize::try_from(MAX_FILE_BYTES).expect("the limit fits") + 1];
    fs::write(&file, big).expect("seed an oversized file");
    let error = workspace
        .read_file(&file)
        .expect_err("an oversized file must be rejected");
    assert!(
        matches!(error, WorkspaceError::FileTooLarge { .. }),
        "expected FileTooLarge, got {error:?}"
    );
}

#[test]
fn oversized_writes_are_rejected() {
    let (workspace, dir) = granted_dir();
    let text = "x".repeat(usize::try_from(MAX_FILE_BYTES).expect("the limit fits") + 1);
    let error = workspace
        .write_file(&dir.path().join("big.txt"), &text, None)
        .expect_err("an oversized write must be rejected");
    assert!(
        matches!(error, WorkspaceError::FileTooLarge { .. }),
        "expected FileTooLarge, got {error:?}"
    );
}

#[test]
fn a_stale_modified_token_conflicts() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("a.txt");
    let written = workspace
        .write_file(&file, "one", None)
        .expect("initial write");
    let stale = format!("{}-stale", written.token);
    let error = workspace
        .write_file(&file, "two", Some(&stale))
        .expect_err("a stale token must conflict");
    assert!(
        matches!(error, WorkspaceError::ModifiedConflict),
        "expected ModifiedConflict, got {error:?}"
    );
    let rewritten = workspace
        .write_file(&file, "two", Some(&written.token))
        .expect("the fresh token writes");
    assert_eq!(rewritten.text, "two");
}

#[test]
fn a_tokenless_write_to_an_existing_file_conflicts() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("a.txt");
    workspace
        .write_file(&file, "one", None)
        .expect("initial write");
    let error = workspace
        .write_file(&file, "two", None)
        .expect_err("a write with no token over an existing file must conflict");
    assert!(
        matches!(error, WorkspaceError::ModifiedConflict),
        "expected ModifiedConflict, got {error:?}"
    );
}

#[test]
fn the_token_tracks_mtime_and_length_not_write_count() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("t.txt");
    let written = workspace.write_file(&file, "one", None).expect("write");
    let metadata = fs::metadata(&file).expect("metadata");
    // The token is a pure function of full-precision mtime plus length:
    // a same-content rewrite changes it exactly when the filesystem
    // reports a new mtime or length, and never otherwise.
    assert_eq!(Some(written.token.clone()), mtime_token(&metadata));
    let rewritten = workspace
        .write_file(&file, "one", Some(&written.token))
        .expect("same-content rewrite");
    let metadata = fs::metadata(&file).expect("metadata after rewrite");
    assert_eq!(Some(rewritten.token), mtime_token(&metadata));
    // Reading without a write in between re-derives the same token.
    let reread = workspace.read_file(&file).expect("read");
    let again = workspace.read_file(&file).expect("second read");
    assert_eq!(reread.token, again.token);
}

#[test]
fn the_hash_fallback_token_round_trips() {
    let token = hash_token(b"same contents");
    assert_eq!(
        token,
        hash_token(b"same contents"),
        "the fallback token must be stable for identical contents"
    );
    assert!(token.starts_with("h-"), "got {token}");
    assert_ne!(token, hash_token(b"different contents"));
}

#[cfg(unix)]
#[test]
fn colon_named_files_read_and_write_on_unix() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("backup-12:30.log");
    let written = workspace
        .write_file(&file, "ok", None)
        .expect("a colon-named file writes on unix");
    let read = workspace
        .read_file(&file)
        .expect("a colon-named file reads on unix");
    assert_eq!(read.text, "ok");
    assert_eq!(read.token, written.token);
}

#[test]
fn tree_lists_directories_before_files_with_stable_ordering() {
    let (workspace, dir) = granted_dir();
    fs::create_dir(dir.path().join("zeta")).expect("dir");
    fs::create_dir(dir.path().join("alpha")).expect("dir");
    fs::write(dir.path().join("b.txt"), "b").expect("file");
    fs::write(dir.path().join("a.txt"), "a").expect("file");
    let listing = workspace.tree(Some(dir.path())).expect("tree");
    let names: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["alpha", "zeta", "a.txt", "b.txt"]);
    assert_eq!(listing.entries[0].kind, EntryKind::Directory);
    assert_eq!(listing.entries[3].kind, EntryKind::File);
    assert!(
        listing.entries.iter().all(|entry| entry.exists),
        "an enumerated directory entry is on disk by construction"
    );
}

#[test]
fn a_tree_without_a_path_lists_the_granted_roots() {
    let (workspace, dir) = granted_dir();
    let listing = workspace.tree(None).expect("roots listing");
    assert_eq!(listing.path, None);
    assert_eq!(listing.entries.len(), 1);
    let root = simplified(dir.path());
    assert_eq!(listing.entries[0].path, root);
    // A root row shows the folder's own name, not the whole path.
    assert_eq!(
        listing.entries[0].name,
        root.file_name().expect("leaf").to_string_lossy()
    );
    assert_eq!(listing.entries[0].kind, EntryKind::Directory);
    assert!(listing.entries[0].exists, "a live root lists as existing");
}

#[test]
fn the_roots_listing_flags_a_deleted_root_as_missing() {
    let workspace = Workspace::new();
    let kept = tempfile::TempDir::new().expect("tempdir");
    let doomed = tempfile::TempDir::new().expect("tempdir");
    let kept_root = workspace.grant(kept.path()).expect("grant the kept root");
    let doomed_root = workspace
        .grant(doomed.path())
        .expect("grant the doomed root");
    doomed.close().expect("delete the doomed directory");
    let listing = workspace.tree(None).expect("roots listing");
    assert_eq!(listing.entries.len(), 2);
    for entry in &listing.entries {
        if entry.path == doomed_root {
            assert!(!entry.exists, "the deleted root must list as missing");
        } else {
            assert_eq!(entry.path, kept_root);
            assert!(entry.exists, "the live root must list as existing");
        }
    }
}

#[test]
fn a_revoke_removes_the_granted_root() {
    let (workspace, dir) = granted_dir();
    let revoked = workspace.revoke(dir.path()).expect("revoke the grant");
    assert_eq!(revoked, simplified(dir.path()));
    assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
}

#[test]
fn revoking_an_unknown_root_errors() {
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    let error = workspace
        .revoke(dir.path())
        .expect_err("an ungranted root must not revoke");
    assert!(
        matches!(error, WorkspaceError::NotGranted),
        "expected NotGranted, got {error:?}"
    );
}

#[test]
fn a_deleted_root_can_still_be_revoked() {
    let (workspace, dir) = granted_dir();
    let root = simplified(dir.path());
    dir.close().expect("delete the granted directory");
    let revoked = workspace.revoke(&root).expect("revoke the deleted root");
    assert_eq!(revoked, root);
    assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
}

#[test]
fn a_spelling_variant_revokes_the_same_root() {
    let (workspace, dir) = granted_dir();
    let variant = PathBuf::from(format!(
        "{}{}",
        dir.path().display(),
        std::path::MAIN_SEPARATOR
    ));
    let revoked = workspace
        .revoke(&variant)
        .expect("a trailing separator names the same root");
    assert_eq!(revoked, simplified(dir.path()));
    assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
}

#[test]
fn reads_and_writes_under_a_revoked_root_are_rejected() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("notes.txt");
    let written = workspace
        .write_file(&file, "hello", None)
        .expect("write before the revoke");
    workspace.revoke(dir.path()).expect("revoke the grant");
    let error = workspace
        .read_file(&file)
        .expect_err("a read under a revoked root must be rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
    let error = workspace
        .write_file(&file, "later", Some(&written.token))
        .expect_err("a write under a revoked root must be rejected");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}

#[test]
fn a_nested_grant_survives_its_parents_revoke() {
    let workspace = Workspace::new();
    let parent = tempfile::TempDir::new().expect("tempdir");
    let child = parent.path().join("child");
    fs::create_dir(&child).expect("create the nested directory");
    fs::write(parent.path().join("outer.txt"), "outer").expect("seed the parent");
    fs::write(child.join("inner.txt"), "inner").expect("seed the child");
    workspace.grant(parent.path()).expect("grant the parent");
    workspace.grant(&child).expect("grant the child");
    workspace.revoke(parent.path()).expect("revoke the parent");
    assert_eq!(workspace.granted_roots(), vec![simplified(&child)]);
    let read = workspace
        .read_file(&child.join("inner.txt"))
        .expect("the nested grant stays usable");
    assert_eq!(read.text, "inner");
    let error = workspace
        .read_file(&parent.path().join("outer.txt"))
        .expect_err("the parent's own files lose access");
    assert!(
        matches!(error, WorkspaceError::OutsideGrants),
        "expected OutsideGrants, got {error:?}"
    );
}

#[test]
fn a_tree_of_a_file_is_rejected() {
    let (workspace, dir) = granted_dir();
    let file = dir.path().join("a.txt");
    fs::write(&file, "a").expect("seed");
    let error = workspace
        .tree(Some(&file))
        .expect_err("a file cannot be listed");
    assert!(
        matches!(error, WorkspaceError::NotADirectory),
        "expected NotADirectory, got {error:?}"
    );
}
