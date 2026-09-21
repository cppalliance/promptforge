//! Tests for the runner's cancel handle, scopes, and parent-to-child propagation.

use super::*;
use std::time::Duration;
use tokio::sync::oneshot;

/// Compile-time proof that a handle can cross task and thread boundaries and
/// live for the whole program: `tokio::spawn` requires `Send + 'static`, and
/// sharing across arms requires `Sync`.
const fn _assert_auto_traits() {
    const fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<CancelHandle>();
}

#[test]
fn cancel_handle_public_construction_surface() {
    // The public constructors remain usable under `#[non_exhaustive]`.
    let a = CancelHandle::new();
    let b = CancelHandle::default();
    let c = a.clone();
    assert!(!a.is_cancelled() && !b.is_cancelled() && !c.is_cancelled());
    a.cancel();
    assert!(
        a.is_cancelled() && c.is_cancelled(),
        "clones share the flag"
    );
}

#[tokio::test]
async fn pre_cancelled_wait_returns_immediately() {
    // A handle cancelled before any await must resolve at once.
    let handle = CancelHandle::new();
    handle.cancel();
    tokio::time::timeout(Duration::from_secs(1), handle.cancelled())
        .await
        .expect("a pre-cancelled handle resolves immediately");
}

#[tokio::test]
async fn repeated_cancel_is_idempotent() {
    let handle = CancelHandle::new();
    handle.cancel();
    handle.cancel();
    assert!(handle.is_cancelled());
    // Still resolves immediately after a redundant second cancel.
    tokio::time::timeout(Duration::from_secs(1), handle.cancelled())
        .await
        .expect("idempotent cancel keeps the handle resolved");
}

#[tokio::test]
async fn cancel_wakes_waiter() {
    // No sleep: the waiter signals it is about to await via a oneshot, and
    // the no-lost-wakeup contract guarantees a cancel racing the await is
    // still delivered.
    let handle = CancelHandle::new();
    let waiter = handle.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _ = ready_tx.send(());
        waiter.cancelled().await;
    });
    ready_rx.await.expect("waiter signals readiness");
    assert!(!handle.is_cancelled());
    handle.cancel();
    tokio::time::timeout(Duration::from_secs(1), join)
        .await
        .expect("waiter must finish after cancel")
        .expect("join ok");
    assert!(handle.is_cancelled());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn multiple_waiters_all_wake_on_a_single_cancel() {
    let handle = CancelHandle::new();
    let mut joins = Vec::new();
    for _ in 0..8 {
        let waiter = handle.clone();
        joins.push(tokio::spawn(async move { waiter.cancelled().await }));
    }
    handle.cancel();
    for join in joins {
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("every waiter must wake on one cancel")
            .expect("join ok");
    }
}

#[tokio::test]
async fn dropping_a_pending_wait_does_not_panic_or_affect_clones() {
    let handle = CancelHandle::new();
    {
        let waiter = handle.clone();
        let fut = waiter.cancelled();
        drop(fut); // Drop a pending wait future before it resolves.
    }
    assert!(!handle.is_cancelled(), "dropping a waiter changes no state");
    handle.cancel();
    assert!(handle.is_cancelled());
}

#[tokio::test]
async fn a_cloned_handle_propagates_cancel_across_a_spawn_boundary() {
    // The child-propagation case: a clone moved into a spawned task observes
    // a cancel issued on the parent handle.
    let parent = CancelHandle::new();
    let child = parent.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _ = ready_tx.send(());
        child.cancelled().await;
    });
    ready_rx.await.expect("child signals readiness");
    parent.cancel();
    tokio::time::timeout(Duration::from_secs(1), join)
        .await
        .expect("a spawned clone must observe the parent's cancel")
        .expect("join ok");
}

#[tokio::test]
async fn current_reports_absent_and_present_context() {
    // PF-CANCEL-003: an absent cancellation context is representable as
    // `None` (not a silent forever-pending), and an installed scope exposes
    // the explicit handle to pass across a spawn boundary.
    assert!(current().is_none(), "no scope installed => no handle");
    let handle = CancelHandle::new();
    let probe = handle.clone();
    scope(handle, async {
        let got = current().expect("an installed scope exposes its handle");
        assert!(!got.is_cancelled());
        probe.cancel();
        assert!(
            current().expect("still present").is_cancelled(),
            "the exposed handle reflects cancellation"
        );
    })
    .await;
    assert!(
        current().is_none(),
        "the handle is gone after the scope exits"
    );
}

#[tokio::test]
async fn missing_scope_wait_stays_pending() {
    // With no handle installed, `wait_cancelled` never completes.
    let elapsed = tokio::time::timeout(Duration::from_millis(50), wait_cancelled()).await;
    assert!(
        elapsed.is_err(),
        "wait_cancelled must stay pending without an installed scope"
    );
    assert!(
        !is_cancelled(),
        "is_cancelled is false with no installed scope"
    );
}

#[tokio::test]
async fn nested_scopes_use_the_innermost_handle() {
    let outer = CancelHandle::new();
    let inner = CancelHandle::new();
    let inner_probe = inner.clone();
    scope(outer, async move {
        scope(inner, async {
            assert!(!is_cancelled());
            inner_probe.cancel();
            assert!(is_cancelled(), "the innermost scope's handle is observed");
            wait_cancelled().await;
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn cancel_between_check_and_wait_is_not_lost() {
    // The no-lost-wakeup contract through the public API: a waiter that has
    // been polled once (and so is registered) but has not yet parked must
    // still observe a cancel that fires in between.
    let handle = CancelHandle::new();
    let wait = handle.cancelled();
    tokio::pin!(wait);
    // Poll once: the waiter registers and reports pending.
    std::future::poll_fn(|cx| {
        assert!(
            wait.as_mut().poll(cx).is_pending(),
            "the waiter is pending before any cancel"
        );
        std::task::Poll::Ready(())
    })
    .await;
    handle.cancel();
    tokio::time::timeout(Duration::from_secs(1), wait)
        .await
        .expect("a registered waiter must observe a cancel signaled before it awaited");
}

#[test]
fn child_is_independent_until_the_parent_cancels() {
    let parent = CancelHandle::new();
    let child = parent.child();
    assert!(!parent.is_cancelled() && !child.is_cancelled());
    // Cloning a child shares the child's state, not the parent's.
    let child_clone = child.clone();
    child.cancel();
    assert!(child_clone.is_cancelled());
    assert!(!parent.is_cancelled(), "child cancel never reaches up");
}

#[tokio::test]
async fn parent_cancel_propagates_to_child() {
    let parent = CancelHandle::new();
    let child = parent.child();
    parent.cancel();
    assert!(child.is_cancelled(), "parent cancel reaches the child");
    // ... and a waiter on the child resolves.
    tokio::time::timeout(Duration::from_secs(1), child.cancelled())
        .await
        .expect("a child waiter resolves after the parent cancels");
}

#[tokio::test]
async fn child_cancel_leaves_parent_and_sibling_unaffected() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let sibling = parent.child();
    child.cancel();
    assert!(child.is_cancelled());
    assert!(!parent.is_cancelled(), "child cancel must not reach up");
    assert!(
        !sibling.is_cancelled(),
        "child cancel must not reach siblings"
    );
    // The sibling still tracks the parent.
    parent.cancel();
    assert!(sibling.is_cancelled());
}

#[test]
fn grandchild_chain_propagates() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let grandchild = child.child();
    parent.cancel();
    assert!(
        child.is_cancelled() && grandchild.is_cancelled(),
        "cancel propagates down the whole chain"
    );
}

#[test]
fn child_of_pre_cancelled_parent_is_born_cancelled() {
    let parent = CancelHandle::new();
    parent.cancel();
    let child = parent.child();
    assert!(
        child.is_cancelled(),
        "a child minted after the parent's cancel starts cancelled"
    );
}

#[tokio::test]
async fn child_waiters_wake_on_parent_cancel() {
    let parent = CancelHandle::new();
    let child = parent.child();
    let (ready_tx, ready_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _ = ready_tx.send(());
        child.cancelled().await;
    });
    ready_rx.await.expect("waiter signals readiness");
    parent.cancel();
    tokio::time::timeout(Duration::from_secs(1), join)
        .await
        .expect("a waiter on the child must wake when the parent is cancelled")
        .expect("join ok");
}

#[tokio::test]
async fn scope_installs_a_child_observed_through_wait_cancelled() {
    // The orchestrator/subagent pattern from `child()`'s docs: the child is
    // installed with `scope`, and the run-level cancel lands through
    // `wait_cancelled()`.
    let parent = CancelHandle::new();
    let child = parent.child();
    let (ready_tx, ready_rx) = oneshot::channel();
    let done = tokio::spawn(async move {
        scope(child, async {
            let _ = ready_tx.send(());
            wait_cancelled().await;
        })
        .await;
    });
    ready_rx.await.expect("scoped task signals readiness");
    parent.cancel();
    tokio::time::timeout(Duration::from_secs(1), done)
        .await
        .expect("the scoped child must observe the parent's cancel")
        .expect("join ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cancel_never_hangs_a_waiter() {
    // Stress the real method: a cancel raced from another thread against a
    // fresh waiter must always complete. The old lost-wakeup would flake.
    for _ in 0..200 {
        let handle = CancelHandle::new();
        let waiter = handle.clone();
        let join = tokio::spawn(async move { waiter.cancelled().await });
        handle.cancel();
        tokio::time::timeout(Duration::from_secs(1), join)
            .await
            .expect("a waiter racing cancel must never hang")
            .expect("join ok");
    }
}

#[tokio::test]
async fn scope_exposes_handle_to_wait_cancelled() {
    let handle = CancelHandle::new();
    let cancel = handle.clone();
    let (ready_tx, ready_rx) = oneshot::channel();
    let done = tokio::spawn(async move {
        scope(handle, async {
            let _ = ready_tx.send(());
            wait_cancelled().await;
        })
        .await;
    });
    ready_rx.await.expect("scoped task signals readiness");
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), done)
        .await
        .expect("scoped wait must finish")
        .expect("join ok");
}
