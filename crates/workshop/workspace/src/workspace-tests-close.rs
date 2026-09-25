//! Closing the backing at shutdown: an ephemeral workspace has nothing
//! to close and says so quietly, a file-backed one folds its WAL into
//! the file and drops the sidecar, leaving exactly one file behind with
//! every grant in it while the in-memory grants stand, and the
//! registered background task hands the server that close as its
//! shutdown lever.

use super::*;

use workshop_registry::Registry;

use crate::handles::register_tasks;
use crate::workspace_file::WorkspaceFile;

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

#[tokio::test]
async fn close_backing_on_an_ephemeral_workspace_is_a_no_op() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    let root = workspace.grant(dir.path()).expect("grant lands in memory");

    workspace.close_backing().await;
    workspace.close_backing().await;

    assert_eq!(
        workspace.current().await.path,
        None,
        "an ephemeral workspace stays ephemeral"
    );
    assert_eq!(
        workspace.granted_roots(),
        vec![root],
        "closing nothing touches no grant"
    );
}

#[tokio::test]
async fn close_backing_leaves_one_file_and_no_sidecar() {
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

    workspace.close_backing().await;

    assert_eq!(
        names_in(home.path()),
        ["mine.pfwork"],
        "the WAL is folded in and the sidecar is gone"
    );
    assert_eq!(
        workspace.current().await.path,
        None,
        "the workspace is ephemeral once its file is closed"
    );
    assert_eq!(
        workspace.granted_roots(),
        vec![root.clone()],
        "the confinement grants stand; only the mirror was let go"
    );
    let file = WorkspaceFile::open(&file_path)
        .await
        .expect("the closed file reopens on its own");
    let contents = file.contents().await.expect("contents read");
    file.close().await;
    assert_eq!(
        contents
            .grants
            .into_iter()
            .map(|grant| grant.path)
            .collect::<Vec<_>>(),
        vec![root],
        "the grant persisted before the close is in the file"
    );
}

#[tokio::test]
async fn the_registered_task_closes_the_backing_on_shutdown() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let registry = Registry::new();
    let workspace = Workspace::new();
    let _registration = register_tasks(&registry, &workspace);
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");

    let tasks = registry.tasks();
    assert_eq!(tasks.len(), 1, "the subsystem registers one shutdown lever");
    // The server's sequence: spawn with serving, stop inside the graceful
    // shutdown. The spawn starts nothing (the actor already runs); the
    // stop is what closes the file.
    let handle = tasks[0].spawn();
    assert_eq!(
        workspace.current().await.path.as_deref(),
        Some(file_path.as_path()),
        "spawning the task leaves the backing open"
    );

    handle.shutdown().await;

    assert_eq!(
        workspace.current().await.path,
        None,
        "the shutdown lever closed the backing"
    );
    assert_eq!(
        names_in(home.path()),
        ["mine.pfwork"],
        "the file is complete and the sidecar is gone"
    );
}
