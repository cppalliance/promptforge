//! Pull-based snapshots and the never-backwards aggregate for renderers.

use crate::event::OperationId;
use crate::hub::ProgressHub;

/// One node's rendering row.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct NodeSnapshot {
    /// Hierarchical id within the operation.
    pub path: String,
    /// Human-readable label.
    pub label: String,
    /// Sibling-relative weight, proportional to expected duration.
    pub weight: f64,
    /// Current fraction in `0.0..=1.0`; for a parent, the weighted aggregate
    /// of its children.
    pub fraction: f64,
    /// Whether the node emitted its terminal `Finished` event.
    pub finished: bool,
    /// The terminal event's success flag; meaningful only when `finished`.
    pub ok: bool,
}

/// One live operation's rendering rows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct OperationSnapshot {
    /// The operation these rows belong to.
    pub operation: OperationId,
    /// Rows in registration order; parents precede their children.
    pub nodes: Vec<NodeSnapshot>,
}

impl ProgressHub {
    /// Snapshots every live operation, ordered by operation id, each with its
    /// nodes in registration order. An idle hub snapshots to an empty vec.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use promptforge_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let tree = hub.operation();
    /// tree.register("download", 1.0);
    /// let snapshot = hub.snapshot();
    /// assert_eq!(snapshot[0].nodes[0].label, "download");
    /// ```
    #[must_use]
    pub fn snapshot(&self) -> Vec<OperationSnapshot> {
        self.trees()
            .iter()
            .map(|tree| OperationSnapshot {
                operation: tree.operation(),
                nodes: tree.snapshot_rows(),
            })
            .collect()
    }

    /// Picks the label of the highest-weight unfinished leaf across live
    /// trees, for status-bar text. Weights are compared by effective share of
    /// their operation. `None` when every leaf is finished or none exist.
    #[must_use]
    pub fn headline(&self) -> Option<String> {
        self.trees()
            .iter()
            .filter_map(|tree| tree.headline())
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, label)| label)
    }
}

/// A stateful aggregate fraction that never steps backward while operations
/// are live.
///
/// When a tree attaches mid-run, the naive mean across live trees would
/// dilute completed work; the meter holds a high-water mark instead, so the
/// displayed aggregate is monotonic. The mark resets only when the hub goes
/// idle, where [`sample`](Self::sample) returns `None`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use promptforge_progress::{ProgressHub, ProgressMeter};
///
/// let hub = Arc::new(ProgressHub::new());
/// let mut meter = ProgressMeter::new();
/// assert_eq!(meter.sample(&hub), None);
/// let tree = hub.operation();
/// tree.register("leaf", 1.0).set_fraction(0.5);
/// assert_eq!(meter.sample(&hub), Some(0.5));
/// ```
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct ProgressMeter {
    high_water: f64,
}

impl ProgressMeter {
    /// Creates a meter at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Samples the hub: `None` when idle (resetting the high-water mark),
    /// otherwise the monotonic mean of the live trees' aggregate fractions.
    #[must_use]
    pub fn sample(&mut self, hub: &ProgressHub) -> Option<f64> {
        let trees = hub.trees();
        if trees.is_empty() {
            self.high_water = 0.0;
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "live operation counts are far below 2^53"
        )]
        let raw = trees.iter().map(|t| t.fraction()).sum::<f64>() / trees.len() as f64;
        self.high_water = self.high_water.max(raw);
        Some(self.high_water)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn snapshot_orders_operations_by_attach_and_nodes_by_registration() {
        let hub = Arc::new(ProgressHub::new());
        let first = hub.operation();
        let second = hub.operation();
        let _b = first.register("b-leaf", 1.0);
        let _a = first.register("a-leaf", 1.0);
        let _c = second.register("c-leaf", 1.0);
        let snapshot = hub.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].operation, first.operation());
        assert_eq!(snapshot[1].operation, second.operation());
        let labels: Vec<&str> = snapshot[0].nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(
            labels,
            ["b-leaf", "a-leaf"],
            "registration order, not sorted"
        );
    }

    #[test]
    fn snapshot_carries_the_terminal_state() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let done = tree.register("done", 1.0);
        let failed = tree.register("failed", 1.0);
        let _live = tree.register("live", 1.0);
        done.complete();
        failed.fail();
        let nodes = &hub.snapshot()[0].nodes;
        assert!(
            nodes[0].finished && nodes[0].ok,
            "a completed node snapshots as finished and ok"
        );
        assert!(
            nodes[1].finished && !nodes[1].ok,
            "a failed node snapshots as finished and not ok"
        );
        assert!(!nodes[2].finished, "a live node snapshots as unfinished");
    }

    #[test]
    fn headline_picks_the_highest_weight_unfinished_leaf() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let heavy = tree.register("heavy", 3.0);
        let light = tree.register("light", 1.0);
        light.set_fraction(0.5);
        assert_eq!(hub.headline().as_deref(), Some("heavy"));
        heavy.complete();
        assert_eq!(hub.headline().as_deref(), Some("light"));
        light.complete();
        assert_eq!(hub.headline(), None);
    }

    #[test]
    fn aggregate_never_steps_backward_while_operations_are_live() {
        let hub = Arc::new(ProgressHub::new());
        let mut meter = ProgressMeter::new();
        let a = hub.operation();
        a.register("a", 1.0).set_fraction(0.8);
        assert_eq!(meter.sample(&hub), Some(0.8));

        // A tree attaching mid-run dilutes the naive mean to 0.4; the
        // high-water mark holds the line instead.
        let b = hub.operation();
        let lb = b.register("b", 1.0);
        let held = meter.sample(&hub).expect("operations are live");
        assert!(
            held >= 0.8,
            "aggregate must not dilute completed work: {held}"
        );
        lb.set_fraction(1.0);
        let risen = meter.sample(&hub).expect("operations are live");
        assert!(
            risen > 0.8,
            "new work still advances the aggregate: {risen}"
        );

        // The mark resets only when the hub goes idle.
        drop(a);
        drop(b);
        assert_eq!(meter.sample(&hub), None);
        let c = hub.operation();
        c.register("c", 1.0).set_fraction(0.1);
        assert_eq!(meter.sample(&hub), Some(0.1));
    }
}
