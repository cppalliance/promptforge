//! The reporting handle a producer holds for one leaf.

use std::sync::Arc;

use crate::tree::{Node, TreeState};

/// A cheap-to-clone, `Send + Sync` reporting handle for one leaf of a
/// [`ProgressTree`](crate::ProgressTree).
///
/// Clones share the leaf: any clone may report. Reporting touches only
/// atomics plus the hub's broadcast channel, never a lock, so worker-thread
/// reporters cannot block each other. Emission is coalesced: an update is
/// broadcast only when the fraction moved at least 1% or 100 ms elapsed since
/// the last emission, and terminal events are never coalesced.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ProgressHandle {
    tree: Arc<TreeState>,
    slot: usize,
    node: Arc<Node>,
}

impl ProgressHandle {
    pub(crate) fn new(tree: Arc<TreeState>, slot: usize, node: Arc<Node>) -> Self {
        Self { tree, slot, node }
    }

    /// Sets the leaf's fraction, clamped to `0.0..=1.0`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use shared_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let tree = hub.operation();
    /// let leaf = tree.register("download", 1.0);
    /// leaf.set_fraction(0.5);
    /// assert_eq!(leaf.fraction(), 0.5);
    /// ```
    pub fn set_fraction(&self, fraction: f64) {
        self.tree.set_fraction(&self.node, fraction);
    }

    /// Sets the fraction from completed units out of a total, for example
    /// bytes downloaded. A zero total reports 0.0 while nothing is done and
    /// 1.0 once any unit is done.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use shared_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let tree = hub.operation();
    /// let leaf = tree.register("download", 1.0);
    /// leaf.set_units(1, 4);
    /// assert_eq!(leaf.fraction(), 0.25);
    /// ```
    pub fn set_units(&self, done: u64, total: u64) {
        #[expect(
            clippy::cast_precision_loss,
            reason = "unit counts beyond 2^53 lose resolution a display cannot show"
        )]
        let fraction = if total == 0 {
            f64::from(done > 0)
        } else {
            done as f64 / total as f64
        };
        self.set_fraction(fraction);
    }

    /// Forces the fraction to 1.0 and emits the terminal `Finished` event,
    /// bypassing coalescing. Call on every exit path of the leaf's work.
    /// Terminal state is sticky: the first of [`complete`](Self::complete) or
    /// [`fail`](Self::fail) wins and later calls are no-ops.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use shared_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let tree = hub.operation();
    /// let leaf = tree.register("download", 1.0);
    /// leaf.set_fraction(0.3);
    /// leaf.complete();
    /// assert_eq!(leaf.fraction(), 1.0);
    /// ```
    pub fn complete(&self) {
        self.tree.complete(&self.node);
    }

    /// Emits the terminal `Finished { ok: false }` event, bypassing
    /// coalescing, and keeps the leaf's current fraction. Call on every
    /// error exit path of the leaf's work, the failure counterpart of
    /// [`complete`](Self::complete). Terminal state is sticky: the first of
    /// `complete` or `fail` wins and later calls are no-ops.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use shared_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let tree = hub.operation();
    /// let leaf = tree.register("download", 1.0);
    /// leaf.set_fraction(0.3);
    /// leaf.fail();
    /// assert_eq!(leaf.fraction(), 0.3);
    /// ```
    pub fn fail(&self) {
        self.tree.finish(&self.node, false);
    }

    /// The leaf's current fraction in `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        self.node.fraction()
    }

    /// Registers a child subtree under this leaf and returns its handle.
    ///
    /// Once a leaf has children its own fraction is the weighted aggregate of
    /// theirs, and `weight` is the child's share of the parent's expected
    /// duration.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use shared_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let tree = hub.operation();
    /// let model = tree.register("model", 1.0);
    /// let download = model.child("download", 3.0);
    /// download.set_fraction(1.0);
    /// assert_eq!(tree.fraction(), 1.0);
    /// ```
    #[must_use]
    pub fn child(&self, label: &str, weight: f64) -> Self {
        let (slot, node) = self.tree.register(Some(self.slot), label, weight);
        Self::new(Arc::clone(&self.tree), slot, node)
    }
}

#[cfg(test)]
mod tests {
    // Fractions are fixed-point millionths, so equality comparisons are exact.
    #![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

    use std::time::Duration;

    use crate::{EventState, ProgressEvent, ProgressHub};

    use super::*;

    fn begun(rx: &mut tokio::sync::broadcast::Receiver<ProgressEvent>) {
        let event = rx.try_recv().expect("register emits Begun");
        assert!(matches!(event.state, EventState::Begun { .. }));
    }

    #[test]
    fn fraction_is_clamped() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("leaf", 1.0);
        leaf.set_fraction(2.0);
        assert_eq!(leaf.fraction(), 1.0);
        leaf.set_fraction(-1.0);
        assert_eq!(leaf.fraction(), 0.0);
        leaf.set_fraction(f64::NAN);
        assert_eq!(leaf.fraction(), 0.0);
    }

    #[test]
    fn set_units_reports_bytes_done_over_total() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("leaf", 1.0);
        leaf.set_units(1, 4);
        assert_eq!(leaf.fraction(), 0.25);
        leaf.set_units(0, 0);
        assert_eq!(leaf.fraction(), 0.0);
        leaf.set_units(3, 0);
        assert_eq!(leaf.fraction(), 1.0);
    }

    #[tokio::test(start_paused = true)]
    async fn coalesces_intermediate_updates() {
        let hub = Arc::new(ProgressHub::new());
        let mut rx = hub.subscribe();
        let tree = hub.operation();
        let leaf = tree.register("download", 1.0);
        begun(&mut rx);

        // The first update always emits; small moves inside 100 ms do not.
        leaf.set_fraction(0.001);
        assert!(matches!(
            rx.try_recv().expect("first update emits").state,
            EventState::Updated { .. }
        ));
        leaf.set_fraction(0.002);
        leaf.set_fraction(0.003);
        assert!(
            rx.try_recv().is_err(),
            "sub-1% moves within 100 ms coalesce"
        );

        // After 100 ms the next update emits even though the move is tiny.
        tokio::time::advance(Duration::from_millis(100)).await;
        leaf.set_fraction(0.004);
        assert!(matches!(
            rx.try_recv().expect("a stale leaf re-emits").state,
            EventState::Updated { fraction } if fraction == 0.004
        ));

        // A move of 1% or more emits immediately.
        leaf.set_fraction(0.014);
        assert!(matches!(
            rx.try_recv().expect("a 1% move emits at once").state,
            EventState::Updated { fraction } if fraction == 0.014
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_events_are_never_coalesced() {
        let hub = Arc::new(ProgressHub::new());
        let mut rx = hub.subscribe();
        let tree = hub.operation();
        let leaf = tree.register("download", 1.0);
        begun(&mut rx);
        leaf.set_fraction(0.5);
        assert!(matches!(
            rx.try_recv().expect("first update emits").state,
            EventState::Updated { .. }
        ));
        leaf.complete();
        assert!(
            matches!(
                rx.try_recv()
                    .expect("complete emits Finished at once")
                    .state,
                EventState::Finished { ok: true }
            ),
            "Finished must not wait out the coalescing window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fail_emits_a_terminal_event_and_keeps_the_fraction() {
        let hub = Arc::new(ProgressHub::new());
        let mut rx = hub.subscribe();
        let tree = hub.operation();
        let leaf = tree.register("download", 1.0);
        begun(&mut rx);
        leaf.set_fraction(0.5);
        assert!(matches!(
            rx.try_recv().expect("first update emits").state,
            EventState::Updated { .. }
        ));
        leaf.fail();
        assert!(
            matches!(
                rx.try_recv().expect("fail emits Finished at once").state,
                EventState::Finished { ok: false }
            ),
            "a failure terminal must not wait out the coalescing window"
        );
        assert_eq!(leaf.fraction(), 0.5, "a failed leaf keeps its fraction");
    }
}
