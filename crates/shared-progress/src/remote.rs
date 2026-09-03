//! Import of another process's event stream as a local operation.

use std::sync::Arc;

use crate::event::{EventState, OperationId, ProgressEvent};
use crate::hub::ProgressHub;
use crate::tree::TreeState;

/// A hub-attached operation whose leaves are driven by [`ProgressEvent`]s
/// arriving from another process.
///
/// It serves both a long-lived subscription (the gateway's event endpoint)
/// and per-request streams. Applied events are re-broadcast on the local hub
/// under the local operation id, so local subscribers see remote activity
/// without id collisions. Dropping detaches the operation from the hub.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use shared_progress::{EventState, ProgressEvent, ProgressHub, RemoteOperation};
///
/// let hub = Arc::new(ProgressHub::new());
/// let remote = RemoteOperation::attach(&hub);
/// # let event = ProgressEvent::new(
/// #     remote.operation(),
/// #     "download",
/// #     "download",
/// #     EventState::Updated { fraction: 0.5 },
/// # );
/// remote.apply(&event);
/// assert_eq!(hub.snapshot()[0].nodes[0].fraction, 0.5);
/// ```
#[derive(Debug)]
#[non_exhaustive]
#[must_use = "a dropped remote operation detaches from the hub"]
pub struct RemoteOperation {
    hub: Arc<ProgressHub>,
    state: Arc<TreeState>,
}

impl RemoteOperation {
    /// Attaches an empty remote operation to `hub` under a fresh local
    /// operation id.
    pub fn attach(hub: &Arc<ProgressHub>) -> Self {
        let state = Arc::new(TreeState::new(OperationId::next(), hub.sender()));
        hub.attach(Arc::clone(&state));
        Self {
            hub: Arc::clone(hub),
            state,
        }
    }

    /// The local operation id the import is attached under.
    #[must_use]
    pub fn operation(&self) -> OperationId {
        self.state.operation()
    }

    /// Applies one event from the remote stream.
    ///
    /// Unknown paths create their leaf on the fly, linked under the longest
    /// already-known prefix parent: intermediate events are lossy, so a
    /// subscriber can see `Updated` before (or without) `Begun`. Fractions
    /// arrive already coalesced and are re-broadcast verbatim. A `Begun`
    /// weight that is not finite and positive falls back to 1.0, so a
    /// poisoned weight off the wire cannot corrupt the aggregate.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use shared_progress::{EventState, ProgressEvent, ProgressHub, RemoteOperation};
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let remote = RemoteOperation::attach(&hub);
    /// let event = ProgressEvent::new(
    ///     remote.operation(),
    ///     "download",
    ///     "download",
    ///     EventState::Finished { ok: true },
    /// );
    /// remote.apply(&event);
    /// assert_eq!(hub.snapshot()[0].nodes[0].fraction, 1.0);
    /// ```
    pub fn apply(&self, event: &ProgressEvent) {
        let (slot, node) = self.state.ensure_remote(&event.path, &event.label);
        match event.state {
            EventState::Begun { weight } => {
                let weight = if weight.is_finite() && weight > 0.0 {
                    weight
                } else {
                    1.0
                };
                self.state.set_weight(slot, weight);
                self.state.emit_begun(&node, weight);
            }
            EventState::Updated { fraction } => {
                self.state.set_fraction_direct(&node, fraction);
            }
            EventState::Finished { ok } => {
                self.state.finish(&node, ok);
            }
        }
    }
}

impl Drop for RemoteOperation {
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

    fn event(operation: OperationId, path: &str, state: EventState) -> ProgressEvent {
        ProgressEvent::new(operation, path, path, state)
    }

    #[test]
    fn apply_drives_the_local_snapshot() {
        let hub = Arc::new(ProgressHub::new());
        let remote = RemoteOperation::attach(&hub);
        let source = OperationId::next();
        remote.apply(&event(
            source,
            "op/download",
            EventState::Begun { weight: 1.0 },
        ));
        remote.apply(&event(
            source,
            "op/download",
            EventState::Updated { fraction: 0.5 },
        ));
        let snapshot = hub.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].nodes[0].fraction, 0.5);
        remote.apply(&event(
            source,
            "op/download",
            EventState::Finished { ok: true },
        ));
        assert_eq!(hub.snapshot()[0].nodes[0].fraction, 1.0);
    }

    #[test]
    fn apply_reconstructs_hierarchy_from_paths() {
        let hub = Arc::new(ProgressHub::new());
        let remote = RemoteOperation::attach(&hub);
        let source = OperationId::next();
        remote.apply(&event(source, "model", EventState::Begun { weight: 1.0 }));
        remote.apply(&event(
            source,
            "model/download",
            EventState::Begun { weight: 3.0 },
        ));
        remote.apply(&event(
            source,
            "model/verify",
            EventState::Begun { weight: 1.0 },
        ));
        remote.apply(&event(
            source,
            "model/download",
            EventState::Updated { fraction: 1.0 },
        ));
        remote.apply(&event(
            source,
            "model/verify",
            EventState::Updated { fraction: 0.5 },
        ));
        let snapshot = hub.snapshot();
        let parent = &snapshot[0].nodes[0];
        assert_eq!(parent.path, "model");
        assert_eq!(
            parent.fraction, 0.875,
            "the parent aggregates its imported children"
        );
    }

    #[test]
    fn apply_rebroadcasts_under_the_local_operation_id() {
        let hub = Arc::new(ProgressHub::new());
        let mut rx = hub.subscribe();
        let remote = RemoteOperation::attach(&hub);
        let source = OperationId::next();
        remote.apply(&event(
            source,
            "download",
            EventState::Updated { fraction: 0.5 },
        ));
        let seen = rx.try_recv().expect("applied events re-broadcast locally");
        assert_eq!(seen.operation, remote.operation());
        assert!(matches!(seen.state, EventState::Updated { fraction } if fraction == 0.5));
    }

    #[test]
    fn failed_finish_preserves_the_fraction() {
        let hub = Arc::new(ProgressHub::new());
        let mut rx = hub.subscribe();
        let remote = RemoteOperation::attach(&hub);
        let source = OperationId::next();
        remote.apply(&event(
            source,
            "download",
            EventState::Updated { fraction: 0.4 },
        ));
        remote.apply(&event(
            source,
            "download",
            EventState::Finished { ok: false },
        ));
        assert_eq!(
            hub.snapshot()[0].nodes[0].fraction,
            0.4,
            "a failed leaf keeps its fraction instead of being forced to 1.0"
        );
        let _updated = rx.try_recv().expect("Updated re-broadcasts");
        let seen = rx.try_recv().expect("Finished re-broadcasts");
        assert!(
            matches!(seen.state, EventState::Finished { ok: false }),
            "the terminal event carries the failure"
        );
    }

    #[test]
    fn apply_sanitizes_a_poisoned_begun_weight() {
        let hub = Arc::new(ProgressHub::new());
        let remote = RemoteOperation::attach(&hub);
        let source = OperationId::next();
        for (path, weight) in [
            ("nan", f64::NAN),
            ("negative", -3.0),
            ("infinite", f64::INFINITY),
        ] {
            remote.apply(&event(source, path, EventState::Begun { weight }));
        }
        let snapshot = hub.snapshot();
        for node in &snapshot[0].nodes {
            assert_eq!(
                node.weight, 1.0,
                "a non-finite or non-positive weight off the wire falls back to 1.0: {}",
                node.path
            );
        }
    }

    #[test]
    fn drop_detaches_the_remote_operation() {
        let hub = Arc::new(ProgressHub::new());
        let remote = RemoteOperation::attach(&hub);
        remote.apply(&event(
            OperationId::next(),
            "download",
            EventState::Begun { weight: 1.0 },
        ));
        assert_eq!(hub.snapshot().len(), 1);
        drop(remote);
        assert!(
            hub.snapshot().is_empty(),
            "a dropped remote operation must leave the hub's snapshots"
        );
    }
}
