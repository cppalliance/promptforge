//! Grant order and time in memory: save-as writes the grants in the
//! order they were made, each with its own time, whether the workspace
//! was file-backed or ephemeral, and positions loaded from a file
//! survive a save-as unchanged.

use super::*;

use crate::workspace_file::{
    GrantRow, WorkspaceContents, WorkspaceFile, empty_ui_state, now_rfc3339,
};

/// Opens the file at `path` directly, bypassing any `Workspace`, and
/// returns the grant rows it holds in file order.
async fn file_rows(path: &Path) -> Vec<GrantRow> {
    let file = WorkspaceFile::open(path)
        .await
        .expect("the workspace file reopens on its own");
    let contents = file.contents().await.expect("contents read");
    file.close().await;
    contents.grants
}

/// Blocks until the clock has moved past `stamp`, so the next grant's
/// `added_at` (whole seconds) differs from the one that produced it.
async fn wait_past(stamp: &str) {
    while now_rfc3339() == stamp {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Grants `z` then `a` (reverse canonical order, so order-by-path and
/// order-by-grant disagree) with distinct times, and returns their roots.
async fn grant_z_then_a(
    workspace: &Workspace,
    z: &tempfile::TempDir,
    a: &tempfile::TempDir,
) -> (PathBuf, PathBuf) {
    let z_root = workspace
        .grant_and_persist(z.path())
        .await
        .expect("grant z lands");
    // z's `added_at` is the second the grant ran, which is at or before
    // this sample; waiting past the sample moves a's time past z's.
    wait_past(&now_rfc3339()).await;
    let a_root = workspace
        .grant_and_persist(a.path())
        .await
        .expect("grant a lands");
    (z_root, a_root)
}

/// Asserts the saved rows are `z` at position 0 then `a` at position 1,
/// each with its own time.
fn assert_z_then_a(rows: &[GrantRow], z_root: &Path, a_root: &Path) {
    assert_eq!(
        rows.iter().map(|row| row.path.clone()).collect::<Vec<_>>(),
        vec![z_root.to_path_buf(), a_root.to_path_buf()],
        "the new file holds the grants in the order they were made, not by path"
    );
    assert_eq!(
        rows.iter().map(|row| row.position).collect::<Vec<_>>(),
        vec![0, 1],
        "positions count from zero in grant order"
    );
    assert_ne!(
        rows[0].added_at, rows[1].added_at,
        "each grant keeps its own time rather than a shared save-as stamp"
    );
}

/// Two tempdirs whose canonical paths sort as `z` after `a`, plus the
/// parent that holds them, which must outlive the test.
fn z_and_a() -> (tempfile::TempDir, tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::TempDir::new().expect("tempdir");
    let z = tempfile::Builder::new()
        .prefix("z-")
        .tempdir_in(home.path())
        .expect("tempdir z");
    let a = tempfile::Builder::new()
        .prefix("a-")
        .tempdir_in(home.path())
        .expect("tempdir a");
    (home, z, a)
}

#[tokio::test]
async fn save_as_from_a_file_backed_workspace_keeps_grant_order_and_times() {
    let files = tempfile::TempDir::new().expect("tempdir");
    let first_path = files.path().join("first.pfwork");
    let second_path = files.path().join("second.pfwork");
    let (_home, z, a) = z_and_a();
    let workspace = Workspace::new();
    workspace
        .save_as(&first_path)
        .await
        .expect("the first save as creates");

    let (z_root, a_root) = grant_z_then_a(&workspace, &z, &a).await;
    assert!(
        z_root > a_root,
        "the fixture must grant in reverse path order"
    );
    workspace
        .save_as(&second_path)
        .await
        .expect("the second save as creates");
    workspace.close_backing_for_test().await;

    assert_z_then_a(&file_rows(&second_path).await, &z_root, &a_root);
}

#[tokio::test]
async fn save_as_from_an_ephemeral_workspace_keeps_grant_order_and_times() {
    let files = tempfile::TempDir::new().expect("tempdir");
    let path = files.path().join("fresh.pfwork");
    let (_home, z, a) = z_and_a();
    let workspace = Workspace::new();

    let (z_root, a_root) = grant_z_then_a(&workspace, &z, &a).await;
    assert!(
        z_root > a_root,
        "the fixture must grant in reverse path order"
    );
    workspace.save_as(&path).await.expect("save as creates");
    workspace.close_backing_for_test().await;

    assert_z_then_a(&file_rows(&path).await, &z_root, &a_root);
}

#[tokio::test]
async fn replace_all_preserves_stored_positions_through_save_as() {
    let files = tempfile::TempDir::new().expect("tempdir");
    let source_path = files.path().join("source.pfwork");
    let copy_path = files.path().join("copy.pfwork");
    let a = tempfile::TempDir::new().expect("tempdir a");
    let b = tempfile::TempDir::new().expect("tempdir b");
    let a_root = simplified(a.path());
    let b_root = simplified(b.path());
    let authored = vec![
        GrantRow {
            path: b_root.clone(),
            position: 5,
            added_at: "2026-09-16T10:00:00Z".to_string(),
        },
        GrantRow {
            path: a_root.clone(),
            position: 7,
            added_at: "2026-09-16T10:00:07Z".to_string(),
        },
    ];
    let contents = WorkspaceContents {
        name: "source".to_string(),
        grants: authored.clone(),
        window_state: None,
        ui_state: empty_ui_state(),
    };
    WorkspaceFile::create(&source_path, &contents)
        .await
        .expect("the source file creates")
        .close()
        .await;

    let workspace = Workspace::new();
    workspace
        .open_file(&source_path)
        .await
        .expect("the source file opens");
    workspace
        .save_as(&copy_path)
        .await
        .expect("save as creates the copy");
    workspace.close_backing_for_test().await;

    assert_eq!(
        file_rows(&copy_path).await,
        authored,
        "b before a with the stored positions and times intact"
    );
}
