//! The wire vocabulary: what a leaf reports and a hub broadcasts.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Identifies one live operation tree within a process.
///
/// Ids come from a process-local counter, so an id received from a remote
/// process is meaningful only inside that process's event stream; a
/// [`RemoteOperation`](crate::RemoteOperation) re-issues events under a local
/// id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct OperationId(u64);

impl OperationId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// The raw numeric id.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use shared_progress::ProgressHub;
    ///
    /// let hub = Arc::new(ProgressHub::new());
    /// let tree = hub.operation();
    /// assert!(tree.operation().get() > 0);
    /// ```
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "op-{}", self.0)
    }
}

/// One progress observation emitted by a leaf of an operation tree.
///
/// Intermediate (`Updated`) events are lossy: handles coalesce them and slow
/// receivers drop them. Terminal (`Finished`) events are never coalesced, and
/// consumers detect completion only from `Finished`, never from a fraction
/// reaching 1.0.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ProgressEvent {
    /// The operation tree the leaf belongs to.
    pub operation: OperationId,
    /// Hierarchical leaf id within the operation, for example
    /// `local-models/ggml-large-v3/download`.
    pub path: String,
    /// Human-readable leaf label.
    pub label: String,
    /// What the leaf reports.
    pub state: EventState,
}

impl ProgressEvent {
    /// Creates an event. Producers never construct events (handles emit
    /// them); this constructor serves test doubles and remote import.
    #[must_use]
    pub fn new(
        operation: OperationId,
        path: impl Into<String>,
        label: impl Into<String>,
        state: EventState,
    ) -> Self {
        Self {
            operation,
            path: path.into(),
            label: label.into(),
            state,
        }
    }
}

/// The kind of observation a [`ProgressEvent`] carries.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum EventState {
    /// A leaf registered; `weight` is its share of its parent's expected
    /// duration.
    Begun {
        /// Sibling-relative weight, proportional to expected time.
        weight: f64,
    },
    /// The leaf's fraction moved. Lossy: coalesced at the source and
    /// droppable under receiver lag.
    Updated {
        /// The leaf's fraction in `0.0..=1.0`.
        fraction: f64,
    },
    /// The leaf finished. Never coalesced; the only authoritative completion
    /// signal.
    Finished {
        /// Whether the leaf's work succeeded.
        ok: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_are_unique_and_increasing() {
        let a = OperationId::next();
        let b = OperationId::next();
        assert!(a < b, "later ids must sort after earlier ones: {a} vs {b}");
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::*;

    #[test]
    fn progress_event_survives_a_serde_json_round_trip() {
        for state in [
            EventState::Begun { weight: 2.5 },
            EventState::Updated { fraction: 0.25 },
            EventState::Finished { ok: false },
        ] {
            let event = ProgressEvent::new(OperationId::next(), "op/leaf", "leaf", state);
            let json = serde_json::to_string(&event).expect("the event serializes");
            let back: ProgressEvent = serde_json::from_str(&json).expect("the event deserializes");
            assert_eq!(event, back, "the wire shape must round-trip");
        }
    }
}
