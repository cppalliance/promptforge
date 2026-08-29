//! Operation-scoped weighted progress trees.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::broadcast;
use tokio::time::Instant;

use crate::event::{EventState, OperationId, ProgressEvent};
use crate::handle::ProgressHandle;
use crate::hub::ProgressHub;
use crate::render::NodeSnapshot;

/// Fixed-point scale for fractions: millionths, so worker-thread reporters
/// take no locks.
pub(crate) const SCALE: u64 = 1_000_000;
const SCALE_F: f64 = 1_000_000.0;

/// An update is broadcast only when the fraction moved at least 1% or this
/// many milliseconds elapsed since the leaf's last emission.
const COALESCE_STEP: u64 = SCALE / 100;
const COALESCE_MS: u64 = 100;

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=1_000_000 before the cast"
)]
pub(crate) fn to_fixed(fraction: f64) -> u64 {
    if fraction.is_nan() {
        return 0;
    }
    (fraction.clamp(0.0, 1.0) * SCALE_F).round() as u64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "millionths lose nothing a progress display can show"
)]
pub(crate) fn from_fixed(fixed: u64) -> f64 {
    fixed as f64 / SCALE_F
}

/// One node of a tree: a leaf, or a parent whose fraction aggregates its
/// children. The atomics are the hot reporting path; everything else about a
/// node is immutable or lives in the locked [`TreeInner`].
#[derive(Debug)]
pub(crate) struct Node {
    path: String,
    label: String,
    fraction: AtomicU64,
    finished: AtomicBool,
    last_emit_ms: AtomicU64,
    last_emit_fraction: AtomicU64,
}

impl Node {
    fn new(path: String, label: String) -> Self {
        Self {
            path,
            label,
            fraction: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            last_emit_ms: AtomicU64::new(u64::MAX),
            last_emit_fraction: AtomicU64::new(0),
        }
    }

    pub(crate) fn fraction(&self) -> f64 {
        from_fixed(self.fraction.load(Ordering::Relaxed))
    }
}

#[derive(Debug)]
pub(crate) struct NodeSlot {
    node: Arc<Node>,
    parent: Option<usize>,
    children: Vec<usize>,
    weight: f64,
}

#[derive(Debug, Default)]
pub(crate) struct TreeInner {
    slots: Vec<NodeSlot>,
    by_path: HashMap<String, usize>,
}

/// The shared state behind one operation tree. The hub holds one `Arc`, and
/// every handle of the tree holds another, so handles keep reporting (into
/// their own atomics) even after the tree detaches.
#[derive(Debug)]
pub(crate) struct TreeState {
    operation: OperationId,
    started: Instant,
    live: AtomicBool,
    inner: Mutex<TreeInner>,
    events: broadcast::Sender<ProgressEvent>,
}

impl TreeState {
    pub(crate) fn new(operation: OperationId, events: broadcast::Sender<ProgressEvent>) -> Self {
        Self {
            operation,
            started: Instant::now(),
            live: AtomicBool::new(true),
            inner: Mutex::new(TreeInner::default()),
            events,
        }
    }

    /// A lock poisoned by a panicking peer recovers the value rather than
    /// wedging the process (the workspace's steady-state posture).
    fn lock(&self) -> MutexGuard<'_, TreeInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn operation(&self) -> OperationId {
        self.operation
    }

    /// Stops event emission; called before detaching from the hub.
    pub(crate) fn retire(&self) {
        self.live.store(false, Ordering::Relaxed);
    }

    fn emit(&self, node: &Node, state: EventState) {
        if !self.live.load(Ordering::Relaxed) {
            return;
        }
        let event = ProgressEvent {
            operation: self.operation,
            path: node.path.clone(),
            label: node.label.clone(),
            state,
        };
        tracing::trace!(operation = %self.operation, path = %node.path, "progress event");
        // An absent or lagging receiver is not an error: intermediate
        // delivery is lossy by design and snapshots carry the ground truth.
        let _ = self.events.send(event);
    }

    /// Registers a node and emits its `Begun`. Returns the slot index and the
    /// shared node the new handle will report through.
    pub(crate) fn register(
        &self,
        parent: Option<usize>,
        label: &str,
        weight: f64,
    ) -> (usize, Arc<Node>) {
        let (slot, node) = {
            let mut inner = self.lock();
            let path = match parent {
                Some(p) => format!("{}/{label}", inner.slots[p].node.path),
                None => label.to_owned(),
            };
            let node = Arc::new(Node::new(path.clone(), label.to_owned()));
            let slot = inner.slots.len();
            inner.slots.push(NodeSlot {
                node: Arc::clone(&node),
                parent,
                children: Vec::new(),
                weight,
            });
            if let Some(p) = parent {
                inner.slots[p].children.push(slot);
            }
            inner.by_path.insert(path, slot);
            (slot, node)
        };
        self.emit(&node, EventState::Begun { weight });
        (slot, node)
    }

    /// Finds or creates the node for a remote path, linking it under the
    /// longest already-known prefix parent. Remote import tolerates lost
    /// `Begun` events, so any event can be the first sight of a path.
    pub(crate) fn ensure_remote(&self, path: &str, label: &str) -> (usize, Arc<Node>) {
        let mut inner = self.lock();
        if let Some(&slot) = inner.by_path.get(path) {
            return (slot, Arc::clone(&inner.slots[slot].node));
        }
        let parent = path
            .rsplit_once('/')
            .and_then(|(parent_path, _)| inner.by_path.get(parent_path).copied());
        let node = Arc::new(Node::new(path.to_owned(), label.to_owned()));
        let slot = inner.slots.len();
        inner.slots.push(NodeSlot {
            node: Arc::clone(&node),
            parent,
            children: Vec::new(),
            weight: 1.0,
        });
        if let Some(p) = parent {
            inner.slots[p].children.push(slot);
        }
        inner.by_path.insert(path.to_owned(), slot);
        (slot, node)
    }

    pub(crate) fn set_weight(&self, slot: usize, weight: f64) {
        self.lock().slots[slot].weight = weight;
    }

    /// The hot reporting path: atomics plus a coalesced broadcast, no locks.
    pub(crate) fn set_fraction(&self, node: &Node, fraction: f64) {
        let fixed = to_fixed(fraction);
        node.fraction.store(fixed, Ordering::Relaxed);
        let moved =
            fixed.abs_diff(node.last_emit_fraction.load(Ordering::Relaxed)) >= COALESCE_STEP;
        let now_ms = self.elapsed_ms();
        let last_ms = node.last_emit_ms.load(Ordering::Relaxed);
        let elapsed = last_ms == u64::MAX || now_ms.saturating_sub(last_ms) >= COALESCE_MS;
        if !moved && !elapsed {
            return;
        }
        node.last_emit_fraction.store(fixed, Ordering::Relaxed);
        node.last_emit_ms.store(now_ms, Ordering::Relaxed);
        self.emit(
            node,
            EventState::Updated {
                fraction: from_fixed(fixed),
            },
        );
    }

    /// Re-broadcasts a remote `Begun` under the local operation id.
    pub(crate) fn emit_begun(&self, node: &Node, weight: f64) {
        self.emit(node, EventState::Begun { weight });
    }

    /// Remote import: fractions arrive already coalesced, so every one is
    /// stored and re-broadcast without a second coalescing pass.
    pub(crate) fn set_fraction_direct(&self, node: &Node, fraction: f64) {
        let fixed = to_fixed(fraction);
        node.fraction.store(fixed, Ordering::Relaxed);
        node.last_emit_fraction.store(fixed, Ordering::Relaxed);
        node.last_emit_ms
            .store(self.elapsed_ms(), Ordering::Relaxed);
        self.emit(
            node,
            EventState::Updated {
                fraction: from_fixed(fixed),
            },
        );
    }

    /// Forces 1.0 and emits the terminal event, which is never coalesced.
    pub(crate) fn complete(&self, node: &Node) {
        self.finish(node, true);
    }

    pub(crate) fn finish(&self, node: &Node, ok: bool) {
        if ok {
            node.fraction.store(SCALE, Ordering::Relaxed);
            node.last_emit_fraction.store(SCALE, Ordering::Relaxed);
        }
        node.finished.store(true, Ordering::Relaxed);
        node.last_emit_ms
            .store(self.elapsed_ms(), Ordering::Relaxed);
        self.emit(node, EventState::Finished { ok });
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// The weighted aggregate fraction across the tree's top-level nodes.
    pub(crate) fn fraction(&self) -> f64 {
        let inner = self.lock();
        let mut wsum = 0.0;
        let mut acc = 0.0;
        for (i, slot) in inner.slots.iter().enumerate() {
            if slot.parent.is_none() {
                acc += slot.weight * slot_fraction(&inner, i);
                wsum += slot.weight;
            }
        }
        if wsum > 0.0 { acc / wsum } else { 0.0 }
    }

    /// Rendering rows in registration order; parents precede their children.
    pub(crate) fn snapshot_rows(&self) -> Vec<NodeSnapshot> {
        let inner = self.lock();
        (0..inner.slots.len())
            .map(|i| {
                let slot = &inner.slots[i];
                NodeSnapshot {
                    path: slot.node.path.clone(),
                    label: slot.node.label.clone(),
                    weight: slot.weight,
                    fraction: slot_fraction(&inner, i),
                }
            })
            .collect()
    }

    /// The best status-bar candidate in this tree: the unfinished leaf with
    /// the highest effective weight (its share of the whole operation),
    /// returned as `(share, label)`.
    pub(crate) fn headline(&self) -> Option<(f64, String)> {
        let inner = self.lock();
        let root_weight: f64 = inner
            .slots
            .iter()
            .filter(|s| s.parent.is_none())
            .map(|s| s.weight)
            .sum();
        if root_weight <= 0.0 {
            return None;
        }
        let mut best = None;
        for (i, slot) in inner.slots.iter().enumerate() {
            if slot.parent.is_none() {
                headline_walk(&inner, i, slot.weight / root_weight, &mut best);
            }
        }
        best
    }
}

fn slot_fraction(inner: &TreeInner, slot: usize) -> f64 {
    let s = &inner.slots[slot];
    if s.children.is_empty() {
        return from_fixed(s.node.fraction.load(Ordering::Relaxed));
    }
    let mut wsum = 0.0;
    let mut acc = 0.0;
    for &c in &s.children {
        let w = inner.slots[c].weight;
        acc += w * slot_fraction(inner, c);
        wsum += w;
    }
    if wsum > 0.0 { acc / wsum } else { 0.0 }
}

fn headline_walk(inner: &TreeInner, slot: usize, share: f64, best: &mut Option<(f64, String)>) {
    let s = &inner.slots[slot];
    if s.children.is_empty() {
        let fraction = from_fixed(s.node.fraction.load(Ordering::Relaxed));
        if !s.node.finished.load(Ordering::Relaxed) && fraction < 1.0 {
            let replace = best.as_ref().is_none_or(|(w, _)| share > *w);
            if replace {
                *best = Some((share, s.node.label.clone()));
            }
        }
        return;
    }
    let wsum: f64 = s.children.iter().map(|&c| inner.slots[c].weight).sum();
    if wsum <= 0.0 {
        return;
    }
    for &c in &s.children {
        headline_walk(inner, c, share * inner.slots[c].weight / wsum, best);
    }
}

/// An operation-scoped progress tree attached to a [`ProgressHub`].
///
/// The owner of one operation creates the tree, registers every leaf up
/// front, runs its own control flow, and reports through the handles. The
/// tree measures; it does not schedule. Dropping the tree detaches it from
/// the hub, so a panicking operation still unregisters.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use promptforge_progress::ProgressHub;
///
/// let hub = Arc::new(ProgressHub::new());
/// let tree = hub.operation();
/// let download = tree.register("download", 3.0);
/// let verify = tree.register("verify", 1.0);
/// download.set_fraction(1.0);
/// verify.set_fraction(0.5);
/// assert_eq!(tree.fraction(), 0.875);
/// ```
#[derive(Debug)]
#[non_exhaustive]
#[must_use = "a dropped tree detaches from the hub immediately"]
pub struct ProgressTree {
    hub: Arc<ProgressHub>,
    state: Arc<TreeState>,
}

impl ProgressTree {
    pub(crate) fn new(hub: Arc<ProgressHub>, state: Arc<TreeState>) -> Self {
        Self { hub, state }
    }

    /// The tree's operation id within the hub.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use promptforge_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let tree = hub.operation();
    /// let id = tree.operation();
    /// ```
    #[must_use]
    pub fn operation(&self) -> OperationId {
        self.state.operation()
    }

    /// The weighted aggregate fraction across the tree's top-level leaves, in
    /// `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        self.state.fraction()
    }

    /// Registers a top-level leaf and returns its reporting handle.
    ///
    /// `weight` is the leaf's proportional share of the operation's expected
    /// duration: weights track time, not bytes or unit counts.
    #[must_use]
    pub fn register(&self, label: &str, weight: f64) -> ProgressHandle {
        let (slot, node) = self.state.register(None, label, weight);
        ProgressHandle::new(Arc::clone(&self.state), slot, node)
    }
}

impl Drop for ProgressTree {
    fn drop(&mut self) {
        self.state.retire();
        self.hub.detach(self.state.operation());
    }
}

#[cfg(test)]
mod tests {
    // Fractions are fixed-point millionths, so equality comparisons are exact.
    #![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

    use super::*;

    #[test]
    fn aggregate_is_weighted_by_expected_duration() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let a = tree.register("a", 1.0);
        let b = tree.register("b", 3.0);
        a.set_fraction(1.0);
        b.set_fraction(0.5);
        assert_eq!(tree.fraction(), 0.625);
    }

    #[test]
    fn children_aggregate_into_their_parent() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let parent = tree.register("model", 1.0);
        let download = parent.child("download", 3.0);
        let verify = parent.child("verify", 1.0);
        download.set_fraction(1.0);
        verify.set_fraction(0.5);
        assert_eq!(tree.fraction(), 0.875);
    }

    #[test]
    fn completion_forces_one_despite_bad_estimates() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("download", 1.0);
        leaf.set_units(3, 10);
        assert_eq!(tree.fraction(), 0.3);
        leaf.complete();
        assert_eq!(tree.fraction(), 1.0);
    }
}
