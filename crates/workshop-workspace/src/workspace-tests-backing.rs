//! The optional workspace-file backing: what an ephemeral workspace
//! persists (nothing), what a file-backed one persists (every grant
//! mutation and the window state), how open replaces the grant set, how
//! save-as and duplicate switch files, and the zone-two path where a
//! failed persist leaves memory standing.

use super::*;

use crate::workspace::backing::EPHEMERAL_NAME;
use crate::workspace_file::{WindowState, WorkspaceFile, open_database};

/// Opens the file at `path` directly, bypassing any `Workspace`, and
/// returns the grant paths it holds in file order.
async fn file_grants(path: &Path) -> Vec<PathBuf> {
    let file = WorkspaceFile::open(path)
        .await
        .expect("the workspace file reopens on its own");
    let contents = file.contents().await.expect("contents read");
    file.close().await;
    contents
        .grants
        .into_iter()
        .map(|grant| grant.path)
        .collect()
}

/// A window geometry distinguished by `width`.
fn window(width: u32) -> WindowState {
    WindowState {
        width,
        height: 700,
        x: 5,
        y: 6,
        maximized: false,
    }
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

#[tokio::test]
async fn grant_then_revoke_on_a_file_backed_workspace_persist_and_reopen_reflects_both() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let kept = tempfile::TempDir::new().expect("tempdir");
    let dropped = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");

    let kept_root = workspace
        .grant_and_persist(kept.path())
        .await
        .expect("the kept grant lands");
    workspace
        .grant_and_persist(dropped.path())
        .await
        .expect("the dropped grant lands");
    workspace
        .revoke_and_persist(dropped.path())
        .await
        .expect("the revoke lands");
    workspace.close_backing_for_test().await;

    // A fresh workspace knows nothing but what the file says.
    let reopened = Workspace::new();
    reopened
        .open_file(&file_path)
        .await
        .expect("the saved file opens");
    assert_eq!(
        reopened.granted_roots(),
        vec![kept_root.clone()],
        "the grant persisted and the revoke persisted"
    );
    let current = reopened.current().await;
    assert_eq!(current.path.as_deref(), Some(file_path.as_path()));
    assert_eq!(current.name, "mine");
    assert_eq!(
        current.grants,
        vec![GrantEntry {
            path: kept_root,
            exists: true,
        }]
    );
}

#[tokio::test]
async fn grants_on_an_ephemeral_workspace_leave_no_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();

    let root = workspace
        .grant_and_persist(dir.path())
        .await
        .expect("an ephemeral grant lands in memory");
    let saved = workspace
        .put_window_state(window(800))
        .await
        .expect("an ephemeral window save is a no-op, not a failure");

    assert_eq!(workspace.granted_roots(), vec![root.clone()]);
    assert!(!saved, "nothing was written, and the caller is told so");
    assert_eq!(
        names_in(dir.path()),
        Vec::<String>::new(),
        "an ephemeral workspace writes nothing anywhere"
    );
    let current = workspace.current().await;
    assert_eq!(current.path, None, "an ephemeral workspace has no file");
    assert_eq!(current.name, EPHEMERAL_NAME);
    assert!(current.window_state.is_none());
    assert_eq!(
        current.grants,
        vec![GrantEntry {
            path: root,
            exists: true
        }]
    );
}

#[tokio::test]
async fn open_replaces_every_prior_grant_with_the_files() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("theirs.pfwork");
    let theirs = tempfile::TempDir::new().expect("tempdir");
    let mine = tempfile::TempDir::new().expect("tempdir");
    // Author a file holding one grant, then let go of it.
    let author = Workspace::new();
    author.grant(theirs.path()).expect("grant their root");
    author.save_as(&file_path).await.expect("save as creates");
    author.close_backing_for_test().await;

    let workspace = Workspace::new();
    workspace.grant(mine.path()).expect("grant my root");
    workspace
        .open_file(&file_path)
        .await
        .expect("the authored file opens");

    assert_eq!(
        workspace.granted_roots(),
        vec![simplified(theirs.path())],
        "open is wholesale: the prior grant is gone and the file's grant is in"
    );
    let current = workspace.current().await;
    assert_eq!(current.path.as_deref(), Some(file_path.as_path()));
    assert_eq!(current.name, "theirs");
}

#[tokio::test]
async fn a_vanished_root_stays_granted_and_lists_as_missing() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("vanished.pfwork");
    let doomed = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    let doomed_root = workspace
        .grant(doomed.path())
        .expect("grant the doomed root");
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");
    doomed.close().expect("delete the doomed directory");

    let current = workspace.current().await;
    assert_eq!(
        current.grants,
        vec![GrantEntry {
            path: doomed_root.clone(),
            exists: false,
        }],
        "a live workspace flags the vanished root"
    );

    workspace.close_backing_for_test().await;
    let reopened = Workspace::new();
    reopened
        .open_file(&file_path)
        .await
        .expect("a file naming a vanished root still opens");
    assert_eq!(
        reopened.granted_roots(),
        vec![doomed_root.clone()],
        "the vanished root loads rather than being dropped on open"
    );
    assert_eq!(
        reopened.current().await.grants,
        vec![GrantEntry {
            path: doomed_root,
            exists: false,
        }]
    );
}

#[tokio::test]
async fn a_persist_failure_keeps_the_in_memory_grant_and_returns_success() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("closed.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");
    // The actor behind the backing stops; the backing stays installed,
    // so every persist from here on fails with a closed file.
    workspace.close_backing_for_test().await;

    let root = workspace
        .grant_and_persist(dir.path())
        .await
        .expect("a grant whose persist fails still succeeds");
    assert_eq!(
        workspace.granted_roots(),
        vec![root.clone()],
        "the in-memory grant stands"
    );
    assert!(
        file_grants(&file_path).await.is_empty(),
        "the closed file never received the grant"
    );

    workspace
        .revoke_and_persist(dir.path())
        .await
        .expect("a revoke whose persist fails still succeeds");
    assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());

    let current = workspace.current().await;
    assert_eq!(
        current.path.as_deref(),
        Some(file_path.as_path()),
        "the backing is still reported"
    );
    assert_eq!(
        current.name, "closed",
        "an unreadable file degrades to its stem"
    );
    assert!(current.window_state.is_none());
}

#[tokio::test]
async fn save_as_carries_the_current_grants_and_the_previous_window_state() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let first_path = home.path().join("first.pfwork");
    let second_path = home.path().join("second.pfwork");
    let a = tempfile::TempDir::new().expect("tempdir");
    let b = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    let a_root = workspace.grant(a.path()).expect("grant a");
    workspace
        .save_as(&first_path)
        .await
        .expect("the first save as creates");
    let saved = workspace
        .put_window_state(window(1111))
        .await
        .expect("a file-backed window save lands");
    assert!(saved, "the caller is told the write happened");
    let b_root = workspace
        .grant_and_persist(b.path())
        .await
        .expect("grant b persists");

    workspace
        .save_as(&second_path)
        .await
        .expect("the second save as creates");

    let current = workspace.current().await;
    assert_eq!(current.path.as_deref(), Some(second_path.as_path()));
    assert_eq!(current.name, "second");
    assert_eq!(
        current.window_state.map(|state| state.width),
        Some(1111),
        "the window state travels to the new file"
    );
    workspace.close_backing_for_test().await;
    let mut expected = vec![a_root, b_root];
    expected.sort();
    assert_eq!(
        file_grants(&second_path).await,
        expected,
        "the new file holds every current grant"
    );
    let mut in_first = file_grants(&first_path).await;
    in_first.sort();
    assert_eq!(
        in_first, expected,
        "the first file keeps what was persisted into it while it was the backing"
    );
    assert_eq!(
        names_in(home.path()),
        ["first.pfwork", "second.pfwork"],
        "save as creates exactly one more file and closes the first cleanly"
    );
}

#[tokio::test]
async fn duplicate_switches_to_the_copy_and_leaves_the_original_independent() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let original_path = home.path().join("original.pfwork");
    let copy_path = home.path().join("copy.pfwork");
    let a = tempfile::TempDir::new().expect("tempdir");
    let b = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    let a_root = workspace.grant(a.path()).expect("grant a");
    workspace
        .save_as(&original_path)
        .await
        .expect("save as creates");

    workspace
        .duplicate(&copy_path)
        .await
        .expect("the duplicate opens");
    let current = workspace.current().await;
    assert_eq!(
        current.path.as_deref(),
        Some(copy_path.as_path()),
        "duplicate switches to the copy"
    );
    assert_eq!(
        current.name, "original",
        "the copy keeps the original's display name"
    );
    assert_eq!(workspace.granted_roots(), vec![a_root.clone()]);

    let b_root = workspace
        .grant_and_persist(b.path())
        .await
        .expect("grant b persists into the copy");
    workspace.close_backing_for_test().await;

    let mut expected = vec![a_root.clone(), b_root];
    expected.sort();
    let mut copied = file_grants(&copy_path).await;
    copied.sort();
    assert_eq!(copied, expected, "the copy took the later grant");
    assert_eq!(
        file_grants(&original_path).await,
        vec![a_root],
        "the original never saw it"
    );
}

#[tokio::test]
async fn an_ephemeral_duplicate_is_a_save_as() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let path = home.path().join("from-nothing.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    let root = workspace.grant(dir.path()).expect("grant");

    workspace
        .duplicate(&path)
        .await
        .expect("an ephemeral workspace duplicates into a fresh file");

    let current = workspace.current().await;
    assert_eq!(current.path.as_deref(), Some(path.as_path()));
    workspace.close_backing_for_test().await;
    assert_eq!(file_grants(&path).await, vec![root]);
}

#[tokio::test]
async fn an_alien_file_is_refused_and_the_workspace_is_unchanged() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let file_path = home.path().join("mine.pfwork");
    let alien_path = home.path().join("alien.pfwork");
    let mine = tempfile::TempDir::new().expect("tempdir");
    {
        let conn = open_database(&alien_path)
            .await
            .expect("the alien database opens");
        conn.execute_batch("CREATE TABLE notes (body TEXT NOT NULL);")
            .await
            .expect("a foreign table creates");
    }
    let workspace = Workspace::new();
    let root = workspace.grant(mine.path()).expect("grant mine");
    workspace
        .save_as(&file_path)
        .await
        .expect("save as creates");

    let error = workspace
        .open_file(&alien_path)
        .await
        .expect_err("a database without the stamp is refused");

    assert!(
        matches!(error, WorkspaceError::WorkspaceFileRefused { .. }),
        "expected WorkspaceFileRefused, got {error:?}"
    );
    assert_eq!(workspace.granted_roots(), vec![root], "the grants stand");
    assert_eq!(
        workspace.current().await.path.as_deref(),
        Some(file_path.as_path()),
        "the backing stands"
    );
}

#[tokio::test]
async fn opening_a_missing_path_is_not_found() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();

    let error = workspace
        .open_file(&home.path().join("nowhere.pfwork"))
        .await
        .expect_err("a missing file cannot open");

    assert!(
        matches!(error, WorkspaceError::NotFound),
        "expected NotFound, got {error:?}"
    );
    assert_eq!(workspace.current().await.path, None);
}

#[tokio::test]
async fn save_as_onto_an_existing_path_is_refused_as_taken() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let taken = home.path().join("taken.pfwork");
    fs::write(&taken, b"already here").expect("placeholder writes");
    let workspace = Workspace::new();

    let error = workspace
        .save_as(&taken)
        .await
        .expect_err("save as never overwrites");

    assert!(
        matches!(error, WorkspaceError::WorkspaceFileTaken),
        "expected WorkspaceFileTaken, got {error:?}"
    );
    assert_eq!(
        fs::read(&taken).expect("placeholder reads"),
        b"already here"
    );
    assert_eq!(
        workspace.current().await.path,
        None,
        "the workspace stays ephemeral"
    );
}
