//! The switch operations (open, save-as, duplicate, and the shutdown
//! close) run one at a time. Each is two phases, open or create a handle
//! and then swap it in, with awaits between them; left unserialized, two
//! overlapping switches can open two handles to one file (and the second
//! swap's close unlinks the WAL the survivor writes to), cross-apply
//! another file's grants after the backing moved on, or install a
//! backing after the shutdown close took the previous one. A grant takes
//! the same guard, so it cannot straddle a switch; a revoke resolves its
//! path first and then takes the guard for its removal and mirror.

use std::pin::{Pin, pin};
use std::task::Poll;
use std::time::Duration;

use super::*;

use crate::workspace_file::WorkspaceFile;

/// Polls `future` exactly once from the calling task.
async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

/// The `-wal` sidecar the engine keeps beside `path`.
fn wal_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// A workspace file at `path` holding exactly the grants in `roots`,
/// written through its own workspace and closed, so the file is complete
/// on disk with no opener left behind.
async fn file_with_grants(path: &Path, roots: &[&Path]) {
    let writer = Workspace::new();
    writer.save_as(path).await.expect("save as creates");
    for root in roots {
        writer
            .grant_and_persist(root)
            .await
            .expect("the grant lands");
    }
    writer.close_backing().await;
}

/// The grant paths the file at `path` holds, read straight from disk
/// through a fresh handle, sorted.
async fn grants_on_disk(path: &Path) -> Vec<PathBuf> {
    let file = WorkspaceFile::open(path)
        .await
        .expect("the workspace file reopens from disk");
    let contents = file.contents().await.expect("contents read");
    file.close().await;
    let mut grants: Vec<PathBuf> = contents
        .grants
        .into_iter()
        .map(|grant| grant.path)
        .collect();
    grants.sort();
    grants
}

#[tokio::test]
async fn concurrent_opens_of_one_new_file_share_one_handle_and_keep_the_wal() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let a = home.path().join("a.pfwork");
    let b = home.path().join("b.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    file_with_grants(&b, &[]).await;
    let workspace = Workspace::new();
    workspace.save_as(&a).await.expect("save as creates");

    // Both pass the same-file guard before either swaps: without one
    // switch at a time, two handles to `b` exist and the second swap's
    // close of the first unlinks the WAL the second keeps writing to.
    let (first, second) = tokio::join!(workspace.open_file(&b), workspace.open_file(&b));
    first.expect("the first open succeeds");
    second.expect("the second open succeeds");
    assert_eq!(
        workspace.current().await.path.as_deref(),
        Some(b.as_path()),
        "the backing is b"
    );

    let root = workspace
        .grant_and_persist(dir.path())
        .await
        .expect("the grant lands after the opens");
    let live = workspace
        .backing_file_for_test()
        .expect("the backing is installed");
    drop(workspace);
    assert!(
        wal_of(&b).is_file(),
        "the wal sidecar was unlinked from under the live backing"
    );
    live.close().await;
    assert_eq!(
        grants_on_disk(&b).await,
        vec![root],
        "the grant made after the concurrent opens survives on disk"
    );
}

#[tokio::test]
async fn reloading_the_current_file_cannot_outlive_a_switch_to_another() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let a = home.path().join("a.pfwork");
    let b = home.path().join("b.pfwork");
    let in_a = tempfile::TempDir::new().expect("tempdir");
    let in_b = tempfile::TempDir::new().expect("tempdir");
    file_with_grants(&a, &[in_a.path()]).await;
    file_with_grants(&b, &[in_b.path()]).await;
    let expected_a = grants_on_disk(&a).await;
    let expected_b = grants_on_disk(&b).await;

    // The interleaving depends on where each open first yields, so run
    // both orders several times; the invariant must hold after each.
    for round in 0..8 {
        let workspace = Workspace::new();
        workspace.open_file(&a).await.expect("a opens");
        let (reload, switch) = if round % 2 == 0 {
            tokio::join!(workspace.open_file(&a), workspace.open_file(&b))
        } else {
            let (switch, reload) = tokio::join!(workspace.open_file(&b), workspace.open_file(&a));
            (reload, switch)
        };
        reload.expect("reopening the current file succeeds");
        switch.expect("opening the other file succeeds");

        let current = workspace
            .current()
            .await
            .path
            .expect("a backing is installed");
        let expected = if current == a {
            &expected_a
        } else {
            assert_eq!(current, b, "the backing is one of the two files");
            &expected_b
        };
        let mut roots = workspace.granted_roots();
        roots.sort();
        assert_eq!(
            &roots, expected,
            "round {round}: memory holds the grants of the file that is the backing"
        );
        workspace.close_backing().await;
    }
}

#[tokio::test]
async fn a_grant_racing_a_workspace_open_never_answers_success_and_then_loses_the_grant() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let a = home.path().join("a.pfwork");
    let b = home.path().join("b.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    file_with_grants(&b, &[]).await;
    let workspace = Workspace::new();
    workspace.save_as(&a).await.expect("save as creates");

    // Start the grant and wait for its root to reach memory, then run the
    // open as far as the switch guard allows before the grant resumes: to
    // completion when nothing holds the guard, not at all when the grant
    // does. A grant that straddles the open has its root wiped from memory
    // by the open and then mirrored into the file the open installed.
    let mut grant = pin!(workspace.grant_and_persist(dir.path()));
    let mut open = pin!(workspace.open_file(&b));
    assert!(
        poll_once(grant.as_mut()).await.is_pending(),
        "the grant resolves its path on the blocking pool"
    );
    // A grant that fails on the blocking pool never inserts; polling it
    // once at the deadline surfaces its error instead of spinning.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while workspace.granted_roots().is_empty() {
        if tokio::time::Instant::now() >= deadline {
            let outcome = poll_once(grant.as_mut()).await;
            panic!("the grant never reached memory: {outcome:?}");
        }
        tokio::task::yield_now().await;
    }
    let open_ran_first = workspace.switches.try_lock().is_ok();
    let granted = if open_ran_first {
        open.as_mut().await.expect("b opens");
        grant.await
    } else {
        let granted = grant.await;
        open.await.expect("b opens");
        granted
    };
    let root = granted.expect("the grant succeeds");

    let in_memory = workspace.granted_roots().contains(&root);
    workspace.close_backing().await;
    let in_a = grants_on_disk(&a).await.contains(&root);
    let in_b = grants_on_disk(&b).await.contains(&root);
    if open_ran_first {
        assert!(
            in_memory && in_b,
            "the grant answered success after b opened, but b holds it in memory: {in_memory}, on disk: {in_b}"
        );
    } else {
        assert!(
            in_a,
            "the grant answered success while a was open, but a's file does not hold it"
        );
    }
    assert_eq!(
        in_memory, in_b,
        "memory and b's file disagree about the grant"
    );
}

#[tokio::test]
async fn a_revoke_racing_a_workspace_open_leaves_memory_and_the_open_file_agreeing() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let a = home.path().join("a.pfwork");
    let b = home.path().join("b.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    file_with_grants(&b, &[dir.path()]).await;
    let workspace = Workspace::new();
    workspace.save_as(&a).await.expect("save as creates");
    let root = workspace
        .grant_and_persist(dir.path())
        .await
        .expect("the grant lands");

    // Drive the revoke until its root leaves memory, then run the open as
    // far as the switch guard allows before the revoke resumes: to
    // completion when nothing holds the guard, not at all when the revoke
    // does. A revoke that straddles the open has its removal undone by
    // the grants the open loads and then mirrored into the file the open
    // installed. The mirror waits on the file actor, a task on this
    // runtime, so the revoke cannot finish inside one poll.
    let mut revoke = pin!(workspace.revoke_and_persist(&root));
    let mut open = pin!(workspace.open_file(&b));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while workspace.granted_roots().contains(&root) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the revoke never removed its root from memory"
        );
        if let Poll::Ready(outcome) = poll_once(revoke.as_mut()).await {
            panic!("the revoke answered before mirroring its removal: {outcome:?}");
        }
        tokio::task::yield_now().await;
    }
    let open_ran_first = workspace.switches.try_lock().is_ok();
    if open_ran_first {
        open.as_mut().await.expect("b opens");
        revoke.await.expect("the revoke succeeds");
    } else {
        revoke.await.expect("the revoke succeeds");
        open.await.expect("b opens");
    }

    let in_memory = workspace.granted_roots().contains(&root);
    workspace.close_backing().await;
    let in_a = grants_on_disk(&a).await.contains(&root);
    let in_b = grants_on_disk(&b).await.contains(&root);
    if open_ran_first {
        assert!(
            !in_memory && !in_b,
            "the revoke answered success after b opened, but b still holds the root in memory: {in_memory}, on disk: {in_b}"
        );
    } else {
        assert!(
            !in_a,
            "the revoke answered success while a was open, but a's file still holds the root"
        );
    }
    assert_eq!(
        in_memory, in_b,
        "memory and b's file disagree about the root"
    );
}

#[tokio::test]
async fn a_revoke_after_close_backing_is_refused_and_keeps_the_root_granted() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let a = home.path().join("a.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace.save_as(&a).await.expect("save as creates");
    let root = workspace
        .grant_and_persist(dir.path())
        .await
        .expect("the grant lands");

    workspace.close_backing().await;

    let refused = workspace.revoke_and_persist(&root).await;
    assert!(
        matches!(refused, Err(WorkspaceError::WorkspaceFileFailed { .. })),
        "a revoke after the shutdown close must answer the closed mapping, got {refused:?}"
    );
    assert_eq!(
        workspace.granted_roots(),
        vec![root.clone()],
        "the refused revoke removed the root from memory"
    );
    assert_eq!(
        grants_on_disk(&a).await,
        vec![root],
        "the file closed at shutdown keeps the root"
    );
}

#[tokio::test]
async fn a_grant_after_close_backing_is_refused_and_leaves_the_root_ungranted() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let a = home.path().join("a.pfwork");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let workspace = Workspace::new();
    workspace.save_as(&a).await.expect("save as creates");

    workspace.close_backing().await;

    let refused = workspace.grant_and_persist(dir.path()).await;
    assert!(
        matches!(refused, Err(WorkspaceError::WorkspaceFileFailed { .. })),
        "a grant after the shutdown close must answer the closed mapping, got {refused:?}"
    );
    assert_eq!(
        workspace.granted_roots(),
        Vec::<PathBuf>::new(),
        "the refused grant added the root to memory"
    );
    assert_eq!(
        grants_on_disk(&a).await,
        Vec::<PathBuf>::new(),
        "the file closed at shutdown gained the refused grant"
    );
}

#[tokio::test]
async fn a_revoke_by_the_stored_key_lands_after_the_folder_becomes_a_link_elsewhere() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let folder = home.path().join("granted");
    let elsewhere = home.path().join("elsewhere");
    fs::create_dir(&folder).expect("create the granted folder");
    fs::create_dir(&elsewhere).expect("create the link target");
    let workspace = Workspace::new();
    let root = workspace.grant(&folder).expect("grant the folder");

    fs::remove_dir(&folder).expect("remove the granted folder");
    if !jail::link_dir(&elsewhere, &folder) {
        jail::symlink_unavailable(
            std::env::var_os("CI").is_some(),
            "directory link creation failed",
        );
        return;
    }

    // The stored key now canonicalizes to the link's target, which is not
    // granted; only the literal key names the grant the listing shows.
    let revoked = workspace
        .revoke_and_persist(&root)
        .await
        .expect("the stored key revokes its grant");
    assert_eq!(revoked, root);
    assert_eq!(workspace.granted_roots(), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn a_switch_after_close_backing_is_refused_and_opens_nothing() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let a = home.path().join("a.pfwork");
    let b = home.path().join("b.pfwork");
    let c = home.path().join("c.pfwork");
    let d = home.path().join("d.pfwork");
    file_with_grants(&b, &[]).await;
    let workspace = Workspace::new();
    workspace.save_as(&a).await.expect("save as creates");

    workspace.close_backing().await;

    let refused = |result: Result<(), WorkspaceError>, what: &str| {
        assert!(
            matches!(result, Err(WorkspaceError::WorkspaceFileFailed { .. })),
            "{what} after the shutdown close must answer the closed mapping"
        );
    };
    refused(workspace.open_file(&b).await, "open");
    refused(workspace.save_as(&c).await, "save as");
    refused(workspace.duplicate(&d).await, "duplicate");

    assert_eq!(
        workspace.current().await.path,
        None,
        "no switch installed a backing after the close"
    );
    assert!(!c.exists(), "save as after the close created a file");
    assert!(!d.exists(), "duplicate after the close created a file");
    assert!(
        !wal_of(&b).exists(),
        "open after the close left b's wal sidecar behind"
    );
}

#[tokio::test]
async fn close_backing_waits_for_an_in_flight_open() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let a = home.path().join("a.pfwork");
    let b = home.path().join("b.pfwork");
    file_with_grants(&a, &[]).await;
    file_with_grants(&b, &[]).await;

    // Queue the open and then the close behind a held guard, so the open
    // is provably in flight when the close is issued; releasing hands
    // the guard over in arrival order. The open must complete first and
    // the close must then take `b`, leaving the workspace ephemeral with
    // no sidecar beside either file. A close that did not wait would
    // take `a`, and the open would then install `b` for nobody to close.
    let workspace = Workspace::new();
    workspace.open_file(&a).await.expect("a opens");
    let held = workspace.hold_switches_for_test().await;
    let opener = workspace.clone();
    let target = b.clone();
    let open = tokio::spawn(async move { opener.open_file(&target).await });
    tokio::task::yield_now().await;
    let closer = workspace.clone();
    let close = tokio::spawn(async move { closer.close_backing().await });
    tokio::task::yield_now().await;
    drop(held);

    open.await
        .expect("the open task completes")
        .expect("the open queued ahead of the close completes");
    close.await.expect("the close task completes");

    assert_eq!(
        workspace.current().await.path,
        None,
        "a backing outlived the shutdown close"
    );
    assert!(
        !wal_of(&b).exists(),
        "b's wal sidecar remains after the shutdown close"
    );
    assert!(!wal_of(&a).exists(), "a's wal sidecar remains");
}
