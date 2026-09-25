//! Mutations and duplication: grant ordering by `position`, window-state
//! overwrite, duplicate independence and completeness, sibling copying,
//! and the closed-actor failure mode.

use std::time::Duration;

use super::*;

/// A grant row for `path`; the position is deliberately wrong so a test
/// proves the file assigns its own.
fn grant(path: &str) -> GrantRow {
    GrantRow {
        path: PathBuf::from(path),
        position: 99,
        added_at: "2026-09-16T11:00:00Z".to_string(),
    }
}

/// A contents value with no grants and no window.
fn empty_contents(name: &str) -> WorkspaceContents {
    WorkspaceContents {
        name: name.to_string(),
        grants: Vec::new(),
        window_state: None,
        ui_state: empty_ui_state(),
    }
}

/// The grant paths of `contents` as strings, in file order.
fn paths_of(contents: &WorkspaceContents) -> Vec<String> {
    contents
        .grants
        .iter()
        .map(|grant| grant.path.to_string_lossy().into_owned())
        .collect()
}

/// The grant positions of `contents`, in file order.
fn positions_of(contents: &WorkspaceContents) -> Vec<u32> {
    contents.grants.iter().map(|grant| grant.position).collect()
}

/// The five window fields as a tuple, for equality assertions.
fn fields_of(state: WindowState) -> (u32, u32, i32, i32, bool) {
    (state.width, state.height, state.x, state.y, state.maximized)
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

/// Opens `path`, reads its contents, and closes it again.
async fn reopen_contents(path: &Path) -> WorkspaceContents {
    let file = WorkspaceFile::open(path).await.expect("the file reopens");
    let contents = file.contents().await.expect("contents read");
    file.close().await;
    contents
}

#[tokio::test]
async fn three_grants_reopen_in_insertion_order_by_position() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("order.pfwork");
    let file = WorkspaceFile::create(&path, &empty_contents("Order"))
        .await
        .expect("creates");

    for root in ["C:\\zebra", "C:\\apple", "C:\\mango"] {
        file.add_grant(grant(root)).await.expect("grant persists");
    }
    file.close().await;

    let contents = reopen_contents(&path).await;
    assert_eq!(paths_of(&contents), ["C:\\zebra", "C:\\apple", "C:\\mango"]);
    assert_eq!(
        positions_of(&contents),
        [0, 1, 2],
        "the file assigns one past its maximum, not the row's own position"
    );
}

#[tokio::test]
async fn removing_the_middle_grant_leaves_the_others_positions_unchanged() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("remove.pfwork");
    let file = WorkspaceFile::create(&path, &empty_contents("Remove"))
        .await
        .expect("creates");
    for root in ["C:\\zebra", "C:\\apple", "C:\\mango"] {
        file.add_grant(grant(root)).await.expect("grant persists");
    }

    file.remove_grant(Path::new("C:\\apple"))
        .await
        .expect("removal persists");
    let after_removal = file.contents().await.expect("contents read");
    assert_eq!(paths_of(&after_removal), ["C:\\zebra", "C:\\mango"]);
    assert_eq!(
        positions_of(&after_removal),
        [0, 2],
        "removal never renumbers the survivors"
    );

    file.add_grant(grant("C:\\pear"))
        .await
        .expect("grant persists");
    file.close().await;

    let contents = reopen_contents(&path).await;
    assert_eq!(paths_of(&contents), ["C:\\zebra", "C:\\mango", "C:\\pear"]);
    assert_eq!(
        positions_of(&contents),
        [0, 2, 3],
        "a later grant takes one past the maximum, not the freed slot"
    );
}

#[tokio::test]
async fn window_state_round_trips_and_overwrites() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("window.pfwork");
    let file = WorkspaceFile::create(&path, &empty_contents("Window"))
        .await
        .expect("creates");
    let first = WindowState {
        width: 1024,
        height: 768,
        x: 10,
        y: 20,
        maximized: false,
    };
    let second = WindowState {
        width: 1920,
        height: 1080,
        x: 0,
        y: 0,
        maximized: true,
    };

    file.put_window_state(first).await.expect("first save");
    let saved = file.contents().await.expect("contents read");
    assert_eq!(
        saved.window_state.map(fields_of),
        Some(fields_of(first)),
        "the first save lands"
    );

    file.put_window_state(second).await.expect("second save");
    file.close().await;

    let contents = reopen_contents(&path).await;
    assert_eq!(
        contents.window_state.map(fields_of),
        Some(fields_of(second)),
        "the second save replaces the first rather than failing on the key"
    );
}

#[tokio::test]
async fn duplicate_then_mutate_leaves_the_original_and_the_copy_independent() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let original_path = dir.path().join("original.pfwork");
    let copy_path = dir.path().join("copy.pfwork");
    let original = WorkspaceFile::create(&original_path, &sample_contents())
        .await
        .expect("creates");

    let copy = original
        .duplicate_to(&copy_path)
        .await
        .expect("the duplicate opens");
    assert_eq!(copy.path(), copy_path.as_path());

    original
        .add_grant(grant("C:\\only-in-original"))
        .await
        .expect("grant persists");
    copy.add_grant(grant("C:\\only-in-copy"))
        .await
        .expect("grant persists");
    copy.put_window_state(WindowState {
        width: 1,
        height: 1,
        x: 1,
        y: 1,
        maximized: true,
    })
    .await
    .expect("window persists");
    original.close().await;
    copy.close().await;

    let original_contents = reopen_contents(&original_path).await;
    let copy_contents = reopen_contents(&copy_path).await;
    assert_eq!(
        paths_of(&original_contents),
        [
            "C:\\projects\\zebra",
            "C:\\projects\\apple",
            "C:\\only-in-original"
        ]
    );
    assert_eq!(
        paths_of(&copy_contents),
        [
            "C:\\projects\\zebra",
            "C:\\projects\\apple",
            "C:\\only-in-copy"
        ]
    );
    assert!(
        !original_contents
            .window_state
            .map(fields_of)
            .is_some_and(|fields| fields.4),
        "the copy's window save never reaches the original"
    );
    assert_eq!(
        names_in(dir.path()),
        ["copy.pfwork", "original.pfwork"],
        "a same-folder duplicate is exactly one more file"
    );
}

#[tokio::test]
async fn a_duplicate_taken_with_uncheckpointed_writes_contains_them() {
    let source_dir = tempfile::TempDir::new().expect("tempdir");
    let target_dir = tempfile::TempDir::new().expect("tempdir");
    let source_path = source_dir.path().join("live.pfwork");
    let target_path = target_dir.path().join("snapshot.pfwork");
    let source = WorkspaceFile::create(&source_path, &empty_contents("Live"))
        .await
        .expect("creates");
    // These writes sit in the WAL until a checkpoint folds them into the
    // main file; a copy of the main file alone would miss them.
    for root in ["C:\\one", "C:\\two", "C:\\three"] {
        source.add_grant(grant(root)).await.expect("grant persists");
    }

    let copy = source
        .duplicate_to(&target_path)
        .await
        .expect("the duplicate opens");
    let contents = copy.contents().await.expect("contents read");
    copy.close().await;
    source.close().await;

    assert_eq!(paths_of(&contents), ["C:\\one", "C:\\two", "C:\\three"]);
    assert_eq!(
        names_in(target_dir.path()),
        ["snapshot.pfwork"],
        "the copy is one complete file with no wal sidecar beside it"
    );
}

#[tokio::test]
async fn duplicate_copies_workspace_siblings_but_not_derived_or_unrelated_entries() {
    let source_dir = tempfile::TempDir::new().expect("tempdir");
    let target_dir = tempfile::TempDir::new().expect("tempdir");
    let source_path = source_dir.path().join("world.pfwork");
    let target_path = target_dir.path().join("world-copy.pfwork");
    fs::create_dir_all(source_dir.path().join("runs").join("r1")).expect("runs dir");
    fs::write(
        source_dir.path().join("runs").join("r1").join("run.db"),
        b"run",
    )
    .expect("run file");
    fs::write(source_dir.path().join("index.db"), b"derived").expect("index file");
    fs::create_dir(source_dir.path().join("photos")).expect("unrelated dir");
    fs::write(source_dir.path().join("photos").join("cat.jpg"), b"cat").expect("unrelated file");
    let source = WorkspaceFile::create(&source_path, &empty_contents("World"))
        .await
        .expect("creates");

    let copy = source
        .duplicate_to(&target_path)
        .await
        .expect("the duplicate opens");
    copy.close().await;
    source.close().await;

    assert_eq!(
        names_in(target_dir.path()),
        ["runs", "world-copy.pfwork"],
        "workspace siblings travel; the derived index and the user's own folders do not"
    );
    assert_eq!(
        fs::read(target_dir.path().join("runs").join("r1").join("run.db")).expect("copied run"),
        b"run"
    );
}

#[tokio::test]
async fn duplicate_refuses_a_destination_folder_that_already_holds_a_sibling() {
    let source_dir = tempfile::TempDir::new().expect("tempdir");
    let target_dir = tempfile::TempDir::new().expect("tempdir");
    let source_path = source_dir.path().join("mine.pfwork");
    let target_path = target_dir.path().join("mine-copy.pfwork");
    fs::create_dir(source_dir.path().join("runs")).expect("source runs dir");
    fs::write(source_dir.path().join("runs").join("mine.db"), b"mine").expect("source run");
    // The destination folder is another workspace's home: its `runs/`
    // must never be merged into or overwritten.
    fs::create_dir(target_dir.path().join("runs")).expect("target runs dir");
    fs::write(target_dir.path().join("runs").join("theirs.db"), b"theirs").expect("their run");
    let source = WorkspaceFile::create(&source_path, &empty_contents("Mine"))
        .await
        .expect("creates");

    let error = source
        .duplicate_to(&target_path)
        .await
        .expect_err("a duplicate never lands beside another workspace's siblings");
    source.close().await;

    assert!(
        matches!(&error, WorkspaceFileError::Io { source } if source.kind() == io::ErrorKind::AlreadyExists),
        "expected an AlreadyExists I/O failure, got {error:?}"
    );
    assert!(
        !target_path.exists(),
        "the refusal happens before the file is written"
    );
    assert_eq!(
        names_in(&target_dir.path().join("runs")),
        ["theirs.db"],
        "the other workspace's runs are untouched"
    );
}

#[tokio::test]
async fn duplicate_refuses_an_existing_destination() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let source_path = dir.path().join("source.pfwork");
    let taken = dir.path().join("taken.pfwork");
    fs::write(&taken, b"already here").expect("placeholder writes");
    let source = WorkspaceFile::create(&source_path, &empty_contents("Source"))
        .await
        .expect("creates");

    let error = source
        .duplicate_to(&taken)
        .await
        .expect_err("duplicate never overwrites");
    source.close().await;

    assert!(
        matches!(&error, WorkspaceFileError::Io { source } if source.kind() == io::ErrorKind::AlreadyExists),
        "expected an AlreadyExists I/O failure, got {error:?}"
    );
    assert_eq!(
        fs::read(&taken).expect("placeholder reads"),
        b"already here"
    );
}

#[tokio::test]
async fn a_closed_actor_answers_a_further_add_grant_with_closed() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("closed.pfwork");
    let file = WorkspaceFile::create(&path, &empty_contents("Closed"))
        .await
        .expect("creates");
    let other_handle = file.clone();

    file.close().await;

    let error = other_handle
        .add_grant(grant("C:\\late"))
        .await
        .expect_err("a stopped actor takes no more work");
    assert!(
        matches!(error, WorkspaceFileError::Closed),
        "expected Closed, got {error:?}"
    );
    assert!(
        reopen_contents(&path).await.grants.is_empty(),
        "the late grant never reached the file"
    );
}

#[tokio::test]
async fn dropping_every_handle_closes_the_actor_and_the_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("dropped.pfwork");
    let file = WorkspaceFile::create(&path, &empty_contents("Dropped"))
        .await
        .expect("creates");
    file.add_grant(grant("C:\\kept"))
        .await
        .expect("grant persists");
    let clone = file.clone();

    drop(file);
    drop(clone);

    // The actor drains and closes on its own task; give it a bounded
    // moment to checkpoint and remove the sidecar.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while names_in(dir.path()) != ["dropped.pfwork"] {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the actor did not close the file after every handle dropped: {:?}",
            names_in(dir.path())
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let contents = reopen_contents(&path).await;
    assert_eq!(paths_of(&contents), ["C:\\kept"]);
}
