//! The synchronous cancellation handle the Engine observes.
//!
//! The Engine is a pure state machine, so it polls a flag between chain
//! steps and from the Lua instruction hook rather than awaiting a
//! cancellation; when the Host cancels, the Harness sets that flag from
//! whichever thread it likes. [`CancelHandle`] is that flag, arranged as a
//! tree so a run-level cancel reaches every task while one task can be
//! cancelled without touching its siblings or its owner.
//!
//! This is the handle the Engine's `RunContext` holds and the one
//! `RunServices` hands a capability; a Host stops a run through
//! `harness::RunControl::cancel`, which sets this flag. A Harness that
//! steps the Engine and must wait on the flag itself awaits
//! [`CancelHandle::cancelled`], a std-only future the cancel itself wakes,
//! in place of a timer that polls the flag.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

#[cfg(test)]
#[path = "cancel-tests.rs"]
mod tests;

/// The source of each [`Cancelled`]'s registration key.
static NEXT_WAITER: AtomicU64 = AtomicU64::new(0);

/// A cloneable cancellation flag in a parent-child tree.
///
/// # Semantics
///
/// - **Shared state.** [`Clone`] produces another handle over the *same*
///   flag. Cancelling any clone cancels every clone.
/// - **Downward propagation.** [`child`](Self::child) mints a handle that
///   reports cancelled when its own flag is set *or* any ancestor's is. A
///   child's cancel reaches only that child and its descendants. Children
///   nest to any depth. A child minted after its parent is cancelled starts
///   cancelled.
/// - **Idempotent and permanent.** Repeated calls to
///   [`cancel`](Self::cancel) have the effect of one, and once
///   [`is_cancelled`](Self::is_cancelled) returns `true`, it always returns
///   `true`.
/// - **Upward links.** A child holds its parent, and every link points
///   upward. The links form a tree, and dropping a handle just releases its
///   reference.
/// - **Awaitable.** [`cancelled`](Self::cancelled) returns a future that
///   the cancel wakes, for a caller that waits on the flag alongside other
///   event sources. Checking the flag stays a plain read. Each waiter
///   stores one waker on every node up its chain. A cancel drops the
///   wakers it fires, and a waiter removes its own when it is dropped.
///
/// Reading walks the ancestor chain, one atomic load per level. The chain is
/// as deep as the run's task nesting, which the Engine caps, so a check from
/// the Engine's Lua instruction hook stays a handful of loads.
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
    /// a descendant, one per waiter key: a waiter registers on every node
    /// up its chain, since a cancel anywhere on the chain completes it,
    /// and a node holds wakers (never handles), so the tree still has no
    /// reference cycles. Drained by the cancel that fires them; a waiter
    /// dropped first removes its own entries.
    wakers: Mutex<Vec<(u64, Waker)>>,
}

impl Node {
    /// This node and each of its ancestors, nearest first.
    fn chain(&self) -> impl Iterator<Item = &Node> {
        std::iter::successors(Some(self), |node| node.parent.as_deref())
    }

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

    /// Registers `waker` for waiter `key`, replacing that waiter's earlier
    /// waker unless the two wake the same task, so a future polled many
    /// times leaves one entry and is woken through its latest waker.
    ///
    /// Entries are keyed by waiter rather than deduplicated by
    /// `will_wake`: two waiters polled by one task hand in equivalent
    /// wakers, and dropping one must not remove the entry the other needs.
    fn register(&self, key: u64, waker: &Waker) {
        let mut wakers = self.wakers.lock().unwrap_or_else(PoisonError::into_inner);
        match wakers.iter_mut().find(|(known, _)| *known == key) {
            Some((_, known)) => {
                if !known.will_wake(waker) {
                    known.clone_from(waker);
                }
            }
            None => wakers.push((key, waker.clone())),
        }
    }

    /// Removes waiter `key`'s entry; a no-op once a cancel drained it.
    fn deregister(&self, key: u64) {
        self.wakers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(known, _)| *known != key);
    }
}

/// A future that completes when the handle it came from reports cancelled.
///
/// Returned by [`CancelHandle::cancelled`]. The future is `Unpin` and owns
/// its handle, so the caller can hold it across awaits or select over it
/// alongside other event sources. It waits for as long as the cancel takes,
/// and the cancel that sets the flag is its one wake-up.
#[derive(Debug)]
#[must_use = "futures do nothing unless polled"]
pub struct Cancelled {
    waiter: Waiter,
}

/// A [`Cancelled`]'s handle and registration key. Dropping it removes the
/// key's entries from every node up the chain.
///
/// The `Drop` sits here rather than on `Cancelled` because a `Drop` impl
/// on `Cancelled` would enter the facade's public API listing.
#[derive(Debug)]
struct Waiter {
    handle: CancelHandle,
    key: u64,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        for node in self.handle.inner.chain() {
            node.deregister(self.key);
        }
    }
}

impl Future for Cancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let waiter = &self.waiter;
        // Register before reading the flag, so a cancel landing between
        // the two is observed by the read rather than lost.
        for node in waiter.handle.inner.chain() {
            node.register(waiter.key, cx.waker());
        }
        if waiter.handle.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl CancelHandle {
    /// Creates a root handle with its flag clear.
    ///
    /// The returned handle is independent of any other until it is cloned
    /// or given children.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a new child handle that reports cancelled when this handle or
    /// any ancestor is cancelled. A cancel on the child reaches only the
    /// child and its descendants.
    ///
    /// The usual pattern gives a run the root handle and each task a child
    /// from `root.child()`, so cancelling the run cancels every task.
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
    /// Idempotent and permanent.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        let wakers = std::mem::take(
            &mut *self
                .inner
                .wakers
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for (_, waker) in wakers {
            waker.wake();
        }
    }

    /// Returns a future that completes when this handle reports cancelled.
    ///
    /// The future is ready on its first poll if the handle is already
    /// cancelled. Otherwise it completes when a cancel lands on this handle
    /// or on an ancestor, and that cancel wakes it.
    pub fn cancelled(&self) -> Cancelled {
        Cancelled {
            waiter: Waiter {
                handle: self.clone(),
                key: NEXT_WAITER.fetch_add(1, Ordering::Relaxed),
            },
        }
    }

    /// Returns whether [`cancel`](Self::cancel) has been called on this
    /// handle, any clone, or any ancestor.
    ///
    /// Monotonic: once it returns `true`, it returns `true` on every later
    /// call.
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
