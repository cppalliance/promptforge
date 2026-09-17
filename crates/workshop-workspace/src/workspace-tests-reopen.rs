//! Opening the workspace file that is already the backing: the reload
//! path that must never start a second opener of the same file (turso
//! shares one WAL handle per file process-wide, so a second opener
//! followed by the first one's close unlinks the sidecar the survivor
//! keeps writing to).

use super::*;

use crate::workspace_file::WorkspaceFile;

/// The `-wal` sidecar the engine keeps beside `path`.
fn wal_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// `path` spelled with a redundant `.` segment before its file name, so
/// it names the same file through a different string.
fn dotted(path: &Path) -> PathBuf {
    let parent = path.parent().expect("the file has a parent");
    let name = path.file_name().expect("the file has a name");
    parent.join(".").join(name)
}

/// The grant paths the file at `path` holds, read straight from disk
/// through a fresh handle, in file order.
async fn grants_on_disk(path: &Path) -> Vec<PathBuf> {
    let file = WorkspaceFile::open(path)
        .await
        .expect("the workspace file reopens from disk");
    let contents = file.contents().await.expect("contents read");
    file.close().await;
    contents
        .grants
        .into_iter()
        .map(|grant| grant.path)
        .collect()
}

#[tokio::test]
async fn reopening_the_current_file_keeps_the_wal_and_later_grants() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let one = tempfile::TempDir::new().expect("tempdir");
    let two = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");
    let root_one = workspace
        .grant_and_persist(one.path())
        .await
        .expect("the first grant lands");

    workspace
        .open_file(&file_path)
        .await
        .expect("reopening the current file succeeds");
    workspace
        .open_file(&dotted(&file_path))
        .await
        .expect("reopening the current file through another spelling succeeds");
    let root_two = workspace
        .grant_and_persist(two.path())
        .await
        .expect("the second grant lands after the reopen");
    assert_eq!(
        workspace.current().await.path.as_deref(),
        Some(file_path.as_path()),
        "the backing path is the one first opened, not the respelling"
    );

    // Hold one handle across the drop so the actor is provably live
    // (dropping the last handle ends its `recv` loop and closes the
    // database at the next yield). With the backing still open, the
    // sidecar must be on disk: the reopens above must not have unlinked
    // it from under the surviving actor.
    let live = workspace
        .backing_file_for_test()
        .expect("the backing is installed");
    drop(workspace);
    assert!(
        wal_of(&file_path).is_file(),
        "the wal sidecar was unlinked from under the live backing"
    );
    // Close the sole opener before reading from disk, so the on-disk
    // read never overlaps a pending close from another handle.
    live.close().await;
    let mut expected = vec![root_one, root_two];
    expected.sort();
    let mut on_disk = grants_on_disk(&file_path).await;
    on_disk.sort();
    assert_eq!(
        on_disk, expected,
        "both grants survive: the one before the reopen and the one after"
    );
}

#[tokio::test]
async fn reopening_the_current_file_starts_no_second_actor() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let workspace = Workspace::new();
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");
    let before = workspace
        .backing_file_for_test()
        .expect("save as installed a backing");

    workspace
        .open_file(&file_path)
        .await
        .expect("reopening the current file succeeds");

    let after = workspace
        .backing_file_for_test()
        .expect("the backing is still installed");
    assert!(
        before.is_same_handle(&after),
        "reopening the current file replaced the backing with a second opener"
    );
}

#[tokio::test]
async fn reopening_the_current_file_reapplies_its_grants() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");
    let root = workspace
        .grant_and_persist(dir.path())
        .await
        .expect("the grant lands");
    // Memory only: the file still holds the grant.
    workspace
        .revoke(dir.path())
        .expect("the memory revoke lands");
    assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());

    workspace
        .open_file(&file_path)
        .await
        .expect("reopening the current file succeeds");

    assert_eq!(
        workspace.granted_roots(),
        vec![root],
        "the reopen reloads the file's grants wholesale"
    );
}
