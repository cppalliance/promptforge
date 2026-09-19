//! The synchronous cancellation handle the engine observes.
//!
//! The engine is a state machine that performs no I/O and holds no runtime
//! handle, so it cannot await a cancellation: it polls a flag between chain
//! steps and from the Lua instruction hook, and the host that cancels it
//! sets that flag from whichever thread it likes. [`CancelHandle`] is that
//! flag, arranged as a tree so a run-level cancel reaches every task while
//! one task can be cancelled without touching its siblings or its owner.
//!
//! This is the handle `RunContext` will carry once the scheduler stops
//! awaiting the tokio token in [`super`]; until then both live side by side.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
            }),
        }
    }

    /// Marks this handle (and every clone and descendant) cancelled.
    ///
    /// Idempotent and irreversible.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
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
