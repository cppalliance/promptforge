//! Cooperative cancellation for the harness's async session paths.
//!
//! Dropping the outer future on Ctrl-C would abandon a run mid-step, so
//! hosts install a [`CancelHandle`] with [`scope`] and call
//! [`CancelHandle::cancel`] from a Ctrl-C task instead, or `select!` over
//! [`CancelHandle::cancelled`] beside the run's effect channel. This is
//! the tokio-aware token a host waits on; the engine itself observes only
//! the polled flag in `promptforge_api_types::cancel`, and a host bridges
//! the one to the other when it launches a run.

use std::future::Future;

use tokio_util::sync::CancellationToken;

tokio::task_local! {
    static CURRENT: CancelHandle;
}

/// A cloneable flag that wakes waiters when cancelled.
///
/// # Semantics
///
/// - **Shared state / propagation.** [`Clone`] produces another handle over the
///   *same* cancellation state. Cancelling any clone cancels every clone, so a
///   handle can be cloned into spawned tasks (for example a Ctrl-C listener)
///   and each observes the same cancellation.
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
/// use harness_runner::cancel::CancelHandle;
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
    token: CancellationToken,
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

    /// Returns a fresh handle cancelled when this handle (or any ancestor) is
    /// cancelled. Cancelling the child never affects the parent or siblings.
    ///
    /// This is the orchestrator/subagent pattern: the orchestrator holds the
    /// run handle, and each subagent task installs `run_handle.child()` via
    /// [`scope`], so Ctrl-C at the run level cancels every subagent while the
    /// orchestrator can cancel one subagent without touching the rest.
    /// Children nest to any depth - a child's own [`child`](Self::child) is a
    /// grandchild cancelled along with it - with no registry and no reference
    /// cycles.
    #[must_use]
    pub fn child(&self) -> CancelHandle {
        CancelHandle {
            token: self.token.child_token(),
        }
    }

    /// Marks this handle (and every clone) cancelled and wakes every waiter.
    ///
    /// Idempotent and irreversible: calling it again after the first time is a
    /// no-op, and a cancelled handle never becomes uncancelled.
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// Returns whether [`Self::cancel`] has been called on this handle or any
    /// clone.
    ///
    /// Monotonic: once it returns `true` it never again returns `false`.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Completes when this handle (or any clone) is cancelled.
    ///
    /// A cancel that lands between a caller's
    /// [`is_cancelled`](Self::is_cancelled) check and the await is never lost:
    /// the returned future observes the cancellation state however the two
    /// were sequenced. Any number of waiters may await concurrently; all are
    /// woken. Dropping the returned future before it resolves is safe and
    /// affects no other waiter. After cancellation this resolves immediately
    /// every time it is called.
    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }
}

/// Runs `fut` with `cancel` installed for [`wait_cancelled`] on this task.
pub async fn scope<F, T>(cancel: CancelHandle, fut: F) -> T
where
    F: Future<Output = T>,
{
    CURRENT.scope(cancel, fut).await
}

/// Runs `fut` under [`scope`] when a handle is present, or bare when it is
/// not - the explicit-cancel install shared by every entry point that takes
/// an optional [`CancelHandle`].
pub async fn maybe_scope<F, T>(cancel: Option<CancelHandle>, fut: F) -> T
where
    F: Future<Output = T>,
{
    match cancel {
        Some(handle) => scope(handle, fut).await,
        None => fut.await,
    }
}

/// Returns the [`CancelHandle`] installed on this task, if any.
///
/// A spawned task (a fanout arm) does NOT inherit the task-local, so code about
/// to cross a spawn boundary reads the current handle here and carries an
/// explicit clone into the new task, where it re-installs it with [`scope`].
/// Returning `Option` makes an absent context representable rather than silently
/// becoming a forever-pending wait.
#[must_use]
pub fn current() -> Option<CancelHandle> {
    CURRENT.try_with(Clone::clone).ok()
}

/// Completes when the task-local [`CancelHandle`] is cancelled.
///
/// When no handle is installed, the future never completes (hosts that do not
/// wire Ctrl-C keep prior behavior).
pub async fn wait_cancelled() {
    match CURRENT.try_with(Clone::clone) {
        Ok(handle) => handle.cancelled().await,
        Err(_) => std::future::pending::<()>().await,
    }
}

/// Reads the task-local [`CancelHandle`] flag without awaiting.
///
/// Returns `false` when no handle is installed. Used by synchronous work (the
/// Lua instruction hook) to poll cancellation cooperatively.
#[must_use]
pub fn is_cancelled() -> bool {
    CURRENT
        .try_with(CancelHandle::is_cancelled)
        .unwrap_or(false)
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "the suite spawns bare waiter tasks to prove cross-task wake-ups; no effect is performed"
)]
#[path = "cancel-tests.rs"]
mod tests;
