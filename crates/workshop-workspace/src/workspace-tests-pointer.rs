//! The last-workspace pointer: written after every successful switch
//! (save-as, open, duplicate), read once at boot by `reopen_last`, and
//! degraded to an ephemeral start - never a failure - when it is
//! missing, corrupt, or names a file that is gone or refused.

use super::*;

use crate::workspace::pointer::LastWorkspacePointer;
use crate::workspace_file::create_alien_database_for_test;

/// The pointer file's bytes, or `None` when it does not exist.
fn pointer_bytes(state_dir: &Path) -> Option<Vec<u8>> {
    fs::read(state_dir.join("last-workspace")).ok()
}

/// The sorted names of every entry directly inside `dir`.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("the directory lists")
        .map(|entry| {
            entry
                .expect("entry reads")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// Authors a workspace file at `path` holding one grant of `root`, then
/// lets go of it so another workspace can open it.
async fn author_file(path: &Path, root: &Path) -> PathBuf {
    let author = Workspace::new();
    let granted = author.grant(root).expect("grant the root");
    author.save_as(path).await.expect("save as creates");
    author.close_backing_for_test().await;
    granted
}

#[tokio::test]
async fn save_as_open_and_duplicate_each_write_the_pointer() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let first = home.path().join("first.pfwork");
    let theirs = home.path().join("theirs.pfwork");
    let copy = home.path().join("copy.pfwork");
    let root = tempfile::TempDir::new().expect("tempdir");
    author_file(&theirs, root.path()).await;
    let workspace = Workspace::with_state_dir(state_dir.path());
    assert_eq!(
        pointer_bytes(state_dir.path()),
        None,
        "an ephemeral workspace has written no pointer"
    );

    workspace.save_as(&first).await.expect("save as creates");
    assert_eq!(
        pointer_bytes(state_dir.path()),
        Some(first.to_string_lossy().into_owned().into_bytes()),
        "save as points at the new file"
    );

    workspace.open_file(&theirs).await.expect("theirs opens");
    assert_eq!(
        pointer_bytes(state_dir.path()),
        Some(theirs.to_string_lossy().into_owned().into_bytes()),
        "open points at the opened file"
    );

    workspace.duplicate(&copy).await.expect("duplicate opens");
    assert_eq!(
        pointer_bytes(state_dir.path()),
        Some(copy.to_string_lossy().into_owned().into_bytes()),
        "duplicate points at the copy"
    );
    workspace.close_backing_for_test().await;
}

#[tokio::test]
async fn a_refused_open_leaves_the_pointer_where_it_was() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let mine = home.path().join("mine.pfwork");
    let alien = home.path().join("alien.pfwork");
    create_alien_database_for_test(&alien)
        .await
        .expect("the alien database creates");
    let workspace = Workspace::with_state_dir(state_dir.path());
    workspace.save_as(&mine).await.expect("save as creates");

    workspace
        .open_file(&alien)
        .await
        .expect_err("the alien file is refused");

    assert_eq!(
        pointer_bytes(state_dir.path()),
        Some(mine.to_string_lossy().into_owned().into_bytes()),
        "only a successful switch moves the pointer"
    );
    workspace.close_backing_for_test().await;
}

#[tokio::test]
async fn a_workspace_without_a_state_dir_writes_no_pointer_and_reopens_nothing() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let path = home.path().join("loose.pfwork");
    let workspace = Workspace::new();
    workspace.save_as(&path).await.expect("save as creates");
    workspace.close_backing_for_test().await;

    assert_eq!(
        names_in(home.path()),
        ["loose.pfwork"],
        "no pointer lands beside the file or anywhere else"
    );
    let fresh = Workspace::new();
    assert!(
        !fresh.reopen_last().await,
        "with nowhere to read a pointer from, nothing reopens"
    );
    assert_eq!(fresh.current().await.path, None);
}

#[tokio::test]
async fn reopen_last_restores_the_pointed_workspace() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let path = home.path().join("mine.pfwork");
    let root = tempfile::TempDir::new().expect("tempdir");
    let granted = author_file(&path, root.path()).await;
    LastWorkspacePointer::new(state_dir.path())
        .write(&path)
        .expect("the pointer writes");

    let workspace = Workspace::with_state_dir(state_dir.path());
    assert!(workspace.reopen_last().await, "the pointed file reopens");

    assert_eq!(workspace.granted_roots(), vec![granted]);
    let current = workspace.current().await;
    assert_eq!(current.path.as_deref(), Some(path.as_path()));
    assert_eq!(current.name, "mine");
    workspace.close_backing_for_test().await;
}

#[tokio::test]
async fn reopen_last_with_no_pointer_stays_ephemeral() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::with_state_dir(state_dir.path());

    assert!(
        !workspace.reopen_last().await,
        "a first launch has no pointer"
    );

    assert_eq!(workspace.current().await.path, None);
    assert_eq!(
        names_in(state_dir.path()),
        Vec::<String>::new(),
        "reading a missing pointer creates nothing"
    );
}

#[tokio::test]
async fn reopen_last_with_corrupt_pointer_bytes_stays_ephemeral() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    fs::write(
        state_dir.path().join("last-workspace"),
        [0xff, 0xfe, 0x00, 0xc3],
    )
    .expect("the corrupt pointer writes");
    let workspace = Workspace::with_state_dir(state_dir.path());

    assert!(
        !workspace.reopen_last().await,
        "non-UTF-8 bytes name nothing"
    );

    assert_eq!(workspace.current().await.path, None);
    assert_eq!(
        LastWorkspacePointer::new(state_dir.path()).read(),
        None,
        "the pointer reads as absent rather than as garbage"
    );
}

#[tokio::test]
async fn reopen_last_with_an_empty_pointer_stays_ephemeral() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    fs::write(state_dir.path().join("last-workspace"), "\r\n").expect("the empty pointer writes");
    let workspace = Workspace::with_state_dir(state_dir.path());

    assert!(!workspace.reopen_last().await, "whitespace names nothing");

    assert_eq!(workspace.current().await.path, None);
}

#[tokio::test]
async fn reopen_last_with_a_vanished_target_stays_ephemeral() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let gone = home.path().join("gone.pfwork");
    LastWorkspacePointer::new(state_dir.path())
        .write(&gone)
        .expect("the pointer writes");
    let workspace = Workspace::with_state_dir(state_dir.path());

    assert!(
        !workspace.reopen_last().await,
        "a missing file cannot reopen"
    );

    assert_eq!(workspace.current().await.path, None);
    assert_eq!(
        names_in(home.path()),
        Vec::<String>::new(),
        "reopen never creates the missing file"
    );
}

#[tokio::test]
async fn reopen_last_with_an_alien_target_stays_ephemeral() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let alien = home.path().join("alien.pfwork");
    create_alien_database_for_test(&alien)
        .await
        .expect("the alien database creates");
    LastWorkspacePointer::new(state_dir.path())
        .write(&alien)
        .expect("the pointer writes");
    let workspace = Workspace::with_state_dir(state_dir.path());

    assert!(
        !workspace.reopen_last().await,
        "a refused file does not reopen"
    );

    assert_eq!(workspace.current().await.path, None);
    assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
}

#[test]
fn the_pointer_round_trips_a_path_and_tolerates_a_trailing_newline() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let pointer = LastWorkspacePointer::new(state_dir.path());
    let target = state_dir.path().join("somewhere").join("mine.pfwork");

    pointer.write(&target).expect("the pointer writes");
    assert_eq!(pointer.read(), Some(target.clone()));

    // A hand-edited pointer with an editor's trailing newline still
    // names the same file.
    fs::write(
        state_dir.path().join("last-workspace"),
        format!("{}\n", target.display()),
    )
    .expect("the edited pointer writes");
    assert_eq!(pointer.read(), Some(target));
    assert_eq!(
        names_in(state_dir.path()),
        ["last-workspace"],
        "the atomic write leaves no temp file behind"
    );
}

#[test]
fn the_pointer_write_creates_a_missing_state_dir() {
    let parent = tempfile::TempDir::new().expect("tempdir");
    let state_dir = parent.path().join("state");
    let pointer = LastWorkspacePointer::new(&state_dir);
    let target = parent.path().join("mine.pfwork");

    pointer
        .write(&target)
        .expect("a first-run state directory is created on demand");

    assert_eq!(pointer.read(), Some(target));
}
