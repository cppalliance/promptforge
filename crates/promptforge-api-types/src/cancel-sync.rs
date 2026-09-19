//! The synchronous cancellation handle the engine observes.
//!
//! The engine is a state machine that performs no I/O and holds no runtime
//! handle, so it cannot await a cancellation: it polls a flag between chain
//! steps and from the Lua instruction hook, and the host that cancels it
//! sets that flag from whichever thread it likes. [`CancelHandle`] is that
//! flag, arranged as a tree so a run-level cancel reaches every task while
//! one task can be cancelled without touching its siblings or its owner.
//!
//! This is the handle the engine's `RunContext` carries and the one
//! `RunServices` hands a capability; the awaitable token in [`super`] stays
//! for hosts that select over cancellation and bridge it to this flag. A
//! host that drives the engine and must wait on the flag itself awaits
//! [`CancelHandle::cancelled`], a std-only future woken by the cancel, so
//! no host has to poll the flag on a timer.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

#[cfg(test)]
#[path = "cancel-sync-tests.rs"]
mod tests;

/// A cloneable cancellation flag in a parent-child tree.
///
/// # Semantics
///
/// - **Shared state.** [`Clone`] produces another handle over the *same*
///   flag. Cancelling any clone cancels every clone.
/// - **Downward propagation.** [`child`](Self::child) mints a handle that
///   reports cancelled when its own flag is set *or* any ancestor's is. A
///   child's cancel never reaches its parent or its siblings. Children nest
///   to any depth; a child minted after its parent's cancel starts cancelled.
/// - **Idempotent and irreversible.** [`cancel`](Self::cancel) is a no-op
///   after the first call, and [`is_cancelled`](Self::is_cancelled) never
///   returns to `false`.
/// - **No registry.** A child holds its parent, never the reverse, so there
///   are no reference cycles and nothing to unregister when a handle drops.
/// - **Awaitable.** [`cancelled`](Self::cancelled) is a future the cancel
///   wakes, for a host that waits on the flag beside its other sources.
///   Polling stays a flag read; waiting costs one waker per node per
///   waiter, dropped when the cancel fires them.
///
/// Reading walks the ancestor chain, one atomic load per level. The chain is
/// as deep as the run's task nesting, which the engine caps, so a poll from
/// the instruction hook stays a handful of loads.
///
/// # Examples
///
/// ```
/// use promptforge_api_types::cancel::sync::CancelHandle;
///
/// let run = CancelHandle::new();
/// let task = run.child();
/// let other = run.child();
///
/// task.cancel();
/// assert!(task.is_cancelled());
/// assert!(!run.is_cancelled() && !other.is_cancelled());
///
/// run.cancel();
/// assert!(other.is_cancelled());
/// ```
#[derive(Clone, Default)]
pub struct CancelHandle {
    inner: Arc<Node>,
}

/// One flag in the tree. `parent` is `None` at the root.
#[derive(Default)]
struct Node {
    cancelled: AtomicBool,
    parent: Option<Arc<Node>>,
    /// The wakers of the [`Cancelled`] futures waiting on this node or on
    /// a descendant: a waiter registers on every node up its chain, since
    /// a cancel anywhere on the chain completes it, and a node holds
    /// wakers (never handles), so the tree still has no reference cycles.
    /// Drained by the cancel that fires them.
    wakers: Mutex<Vec<Waker>>,
}

impl Node {
    fn is_cancelled(&self) -> bool {
        let mut node = self;
        loop {
            if node.cancelled.load(Ordering::Acquire) {
                return true;
            }
            match &node.parent {
                Some(parent) => node = parent,
                None => return false,
            }
        }
    }

    /// Registers `waker` on this node unless an equivalent waker already
    /// waits here, so a future polled many times leaves one entry.
    fn register(&self, waker: &Waker) {
        let mut wakers = self.wakers.lock().unwrap_or_else(PoisonError::into_inner);
        if !wakers.iter().any(|known| known.will_wake(waker)) {
            wakers.push(waker.clone());
        }
    }
}

/// Completes when the handle it was drawn from reports cancelled.
///
/// Returned by [`CancelHandle::cancelled`]. The future is `Unpin` and owns
/// its handle, so a host can hold it across awaits or select over it
/// beside its other sources. It never times out or spins: the cancel that
/// sets the flag wakes it.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct Cancelled {
    handle: CancelHandle,
}

impl Future for Cancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // Register before reading the flag, so a cancel landing between
        // the two is observed by the read rather than lost.
        let mut node = &*self.handle.inner;
        loop {
            node.register(cx.waker());
            match &node.parent {
                Some(parent) => node = parent,
                None => break,
            }
        }
        if self.handle.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl CancelHandle {
    /// Creates a root handle that is not yet cancelled.
    ///
    /// The returned handle is independent of any other until it is cloned
    /// or given children.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a fresh handle cancelled when this handle (or any ancestor) is
    /// cancelled. Cancelling the child never affects the parent or siblings.
    ///
    /// This is the run/task pattern: the run holds the root, each task gets
    /// `root.child()`, so cancelling the run cancels every task while the
    /// scheduler can cancel one task without touching the rest.
    #[must_use]
    pub fn child(&self) -> CancelHandle {
        CancelHandle {
            inner: Arc::new(Node {
                cancelled: AtomicBool::new(false),
                parent: Some(Arc::clone(&self.inner)),
                wakers: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Marks this handle (and every clone and descendant) cancelled and
    /// wakes every [`cancelled`](Self::cancelled) future waiting on it or
    /// on a descendant.
    ///
    /// Idempotent and irreversible.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        let wakers = std::mem::take(
            &mut *self
                .inner
                .wakers
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for waker in wakers {
            waker.wake();
        }
    }

    /// A future that completes when this handle reports cancelled: at once
    /// if it already does, otherwise when a cancel lands on it or on an
    /// ancestor. This is how a host that must wait on the flag waits
    /// without polling it on a timer.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::future::Future;
    /// use std::pin::pin;
    /// use std::task::{Context, Poll, Waker};
    ///
    /// use promptforge_api_types::cancel::sync::CancelHandle;
    ///
    /// let run = CancelHandle::new();
    /// let mut waiting = pin!(run.child().cancelled());
    /// let mut cx = Context::from_waker(Waker::noop());
    /// assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Pending);
    /// run.cancel();
    /// assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Ready(()));
    /// ```
    pub fn cancelled(&self) -> Cancelled {
        Cancelled {
            handle: self.clone(),
        }
    }

    /// Returns whether [`cancel`](Self::cancel) has been called on this
    /// handle, any clone, or any ancestor.
    ///
    /// Monotonic: once it returns `true` it never again returns `false`.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

impl fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut depth = 0_usize;
        let mut node = &*self.inner;
        while let Some(parent) = &node.parent {
            depth += 1;
            node = parent;
        }
        f.debug_struct("CancelHandle")
            .field("cancelled", &self.is_cancelled())
            .field("depth", &depth)
            .finish()
    }
}
