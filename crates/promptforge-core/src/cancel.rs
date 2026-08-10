//! Cooperative cancellation for long-running execute paths.
//!
//! Fanout runs inside [`tokio::task::block_in_place`], so dropping the outer
//! `select!` on Ctrl-C does not stop in-flight arms. Hosts install a
//! [`CancelHandle`] with [`scope`] and call [`CancelHandle::cancel`] from a
//! Ctrl-C task; fanout and model turns poll [`wait_cancelled`].

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

tokio::task_local! {
    static CURRENT: CancelHandle;
}

/// A cloneable flag that wakes waiters when cancelled.
///
/// # Semantics
///
/// - **Shared state / propagation.** [`Clone`] produces another handle over the
///   *same* cancellation state. Cancelling any clone cancels every clone, so a
///   handle can be cloned into spawned tasks (for example fanout arms) and each
///   observes the same cancellation.
/// - **Idempotent.** Calling [`cancel`](Self::cancel) more than once is a no-op
///   after the first call.
/// - **Irreversible.** Once cancelled, a handle never returns to the
///   uncancelled state; [`is_cancelled`](Self::is_cancelled) stays `true` and
///   [`cancelled`](Self::cancelled) resolves immediately forever after.
/// - **Drop.** Dropping a handle (or a pending [`cancelled`](Self::cancelled)
///   future) has no effect on the other clones' state and never panics.
///
/// `#[non_exhaustive]` so the crate can add internal state without a breaking
/// change; construct one with [`CancelHandle::new`] or [`Default`].
///
/// # Examples
///
/// ```
/// use promptforge_core::CancelHandle;
///
/// let handle = CancelHandle::new();
/// assert!(!handle.is_cancelled());
///
/// // A clone shares the same cancellation state (propagation).
/// let child = handle.clone();
/// handle.cancel();
/// assert!(child.is_cancelled());
///
/// // cancel() is idempotent and irreversible.
/// handle.cancel();
/// assert!(handle.is_cancelled());
/// ```
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct CancelHandle {
    cancelled: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelHandle {
    /// Creates a handle that is not yet cancelled.
    ///
    /// The returned handle is independent of any other handle until it is
    /// [`clone`](Clone::clone)d; clones then share its state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks this handle (and every clone) cancelled and wakes every waiter.
    ///
    /// Idempotent and irreversible: calling it again after the first time is a
    /// no-op, and a cancelled handle never becomes uncancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Returns whether [`Self::cancel`] has been called on this handle or any
    /// clone.
    ///
    /// Monotonic: once it returns `true` it never again returns `false`.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Completes when this handle (or any clone) is cancelled.
    ///
    /// Registers this waiter (via tokio's `Notified::enable`) *before* re-reading
    /// the flag, so a [`Self::cancel`] that stores `true` and calls
    /// `notify_waiters()` between the check and the await cannot be lost: the
    /// waiter is already queued and the broadcast wakes it. Any number of waiters
    /// may await concurrently; all are woken. Dropping the returned future before
    /// it resolves is safe and affects no other waiter. After cancellation this
    /// resolves immediately every time it is called.
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Enqueue as a waiter now; any notify_waiters() after this point
            // wakes us, closing the check-then-wait race window.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
            if self.is_cancelled() {
                return;
            }
        }
    }
}

/// Runs `fut` with `cancel` installed for [`wait_cancelled`] on this task.
pub(crate) async fn scope<F, T>(cancel: CancelHandle, fut: F) -> T
where
    F: Future<Output = T>,
{
    CURRENT.scope(cancel, fut).await
}

/// Returns the [`CancelHandle`] installed on this task, if any.
///
/// A spawned task (a fanout arm) does NOT inherit the task-local, so code about
/// to cross a spawn boundary reads the current handle here and carries an
/// explicit clone into the new task, where it re-installs it with [`scope`].
/// Returning `Option` makes an absent context representable rather than silently
/// becoming a forever-pending wait.
pub(crate) fn current() -> Option<CancelHandle> {
    CURRENT.try_with(Clone::clone).ok()
}

/// Completes when the task-local [`CancelHandle`] is cancelled.
///
/// When no handle is installed, the future never completes (hosts that do not
/// wire Ctrl-C keep prior behavior).
pub(crate) async fn wait_cancelled() {
    match CURRENT.try_with(Clone::clone) {
        Ok(handle) => handle.cancelled().await,
        Err(_) => std::future::pending::<()>().await,
    }
}

/// Reads the task-local [`CancelHandle`] flag without awaiting.
///
/// Returns `false` when no handle is installed. Used by synchronous work (the
/// Lua instruction hook) to poll cancellation cooperatively.
pub(crate) fn is_cancelled() -> bool {
    CURRENT
        .try_with(CancelHandle::is_cancelled)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
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
        // the lost-wakeup fix (`Notified::enable`) guarantees a cancel racing the
        // await is still delivered.
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
        // the explicit handle for carrying across a spawn boundary.
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
        // Reproduces the exact wait/notify sequence `cancelled()` uses. A
        // waiter that has passed its flag check and holds a `Notified` future
        // must still observe a cancel that fires before it awaits.
        //
        // Under the OLD sequence (create `notified`, then cancel, then await
        // WITHOUT `enable()`), `notify_waiters()` finds no registered waiter,
        // the permit is dropped, and the final `await` below hangs until the
        // timeout fails. `enable()` registers first, so the wakeup is kept.
        let handle = CancelHandle::new();
        let notified = handle.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        handle.cancel();
        tokio::time::timeout(Duration::from_secs(1), notified)
            .await
            .expect("an enabled waiter must observe a cancel signaled before it awaited");
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
}
