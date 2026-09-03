//! The process-wide broker that operation trees attach to.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::broadcast;

use crate::event::{OperationId, ProgressEvent};
use crate::tree::{ProgressTree, TreeState};

/// Broadcast ring capacity. Receivers that lag past it drop intermediate
/// events, which are lossy by design; snapshots carry the ground truth.
const EVENT_CAPACITY: usize = 1024;

/// One per process: brokers live operation trees to event subscribers and
/// snapshot readers.
///
/// The hub lives in the host's application state for the process lifetime;
/// operations install and remove themselves by their own lifetimes. The empty
/// set is the idle state: subscribers then see only silence and snapshots are
/// empty. The membership lock guards attach, detach, and snapshot walks with
/// microscopic critical sections; leaf fractions are atomics inside the
/// trees, so reporters on worker threads never touch it.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use shared_progress::ProgressHub;
///
/// let hub = Arc::new(ProgressHub::new());
/// assert!(hub.snapshot().is_empty());
/// let tree = hub.operation();
/// assert_eq!(hub.snapshot().len(), 1);
/// drop(tree);
/// assert!(hub.snapshot().is_empty());
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct ProgressHub {
    live: Mutex<HashMap<OperationId, Arc<TreeState>>>,
    events: broadcast::Sender<ProgressEvent>,
}

impl Default for ProgressHub {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressHub {
    /// Creates an empty hub: the idle state, running no operations.
    #[must_use]
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            live: Mutex::new(HashMap::new()),
            events,
        }
    }

    /// Attaches a fresh operation tree and returns its owner handle. The tree
    /// detaches itself when dropped.
    pub fn operation(self: &Arc<Self>) -> ProgressTree {
        let state = Arc::new(TreeState::new(OperationId::next(), self.events.clone()));
        self.lock().insert(state.operation(), Arc::clone(&state));
        tracing::debug!(operation = %state.operation(), "operation attached");
        ProgressTree::new(Arc::clone(self), state)
    }

    /// Subscribes to the event stream of every live and future operation.
    ///
    /// Intermediate events are lossy: a receiver that falls more than the
    /// ring capacity behind drops them. Terminal events are never coalesced
    /// at the source, but a lagging receiver can still drop them; consumers
    /// that must observe completion take it from task join results.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use shared_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let mut events = hub.subscribe();
    /// let tree = hub.operation();
    /// let _leaf = tree.register("download", 1.0);
    /// assert!(events.try_recv().is_ok());
    /// ```
    pub fn subscribe(&self) -> broadcast::Receiver<ProgressEvent> {
        self.events.subscribe()
    }

    /// A lock poisoned by a panicking peer recovers the value rather than
    /// wedging the process (the workspace's steady-state posture).
    pub(crate) fn lock(&self) -> MutexGuard<'_, HashMap<OperationId, Arc<TreeState>>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn attach(&self, state: Arc<TreeState>) {
        self.lock().insert(state.operation(), state);
    }

    pub(crate) fn detach(&self, operation: OperationId) {
        self.lock().remove(&operation);
        tracing::debug!(%operation, "operation detached");
    }

    pub(crate) fn sender(&self) -> broadcast::Sender<ProgressEvent> {
        self.events.clone()
    }

    /// The live trees, ordered by operation id (attach order).
    pub(crate) fn trees(&self) -> Vec<Arc<TreeState>> {
        let mut trees: Vec<Arc<TreeState>> = self.lock().values().cloned().collect();
        trees.sort_by_key(|t| t.operation());
        trees
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detach_on_drop_removes_the_tree_from_snapshots() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let _leaf = tree.register("leaf", 1.0);
        assert_eq!(hub.snapshot().len(), 1);
        drop(tree);
        assert!(
            hub.snapshot().is_empty(),
            "a dropped tree must leave the hub's snapshots"
        );
    }

    #[test]
    fn handles_outliving_their_tree_do_not_emit() {
        let hub = Arc::new(ProgressHub::new());
        let mut rx = hub.subscribe();
        let tree = hub.operation();
        let leaf = tree.register("leaf", 1.0);
        assert!(rx.try_recv().is_ok(), "register emits Begun");
        drop(tree);
        leaf.set_fraction(1.0);
        assert!(
            rx.try_recv().is_err(),
            "a detached tree's handles stay silent"
        );
    }
}
