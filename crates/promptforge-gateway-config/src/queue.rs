//! Waiting-queue settings for concurrency-limited endpoints.
//!
//! `max_depth` is the number of *waiting* requests allowed (not counting
//! in-flight). The runtime admission controller built from these settings
//! lives in the gateway crate.

use serde::Deserialize;

/// Waiting-queue settings shared by every limited endpoint lane.
///
/// `max_depth` counts only requests waiting for a concurrency slot, not
/// requests already admitted (in-flight).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct QueueConfig {
    /// Maximum number of waiting requests before new admits are rejected.
    /// Defaults to 100.
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    /// When true, waiting callers are served round-robin by client key.
    /// Defaults to true.
    #[serde(default = "default_fair_scheduling")]
    pub fair_scheduling: bool,
}

fn default_max_depth() -> usize {
    100
}

fn default_fair_scheduling() -> bool {
    true
}

impl QueueConfig {
    /// Queue settings with the given waiting-queue depth and fairness switch.
    #[must_use]
    pub fn new(max_depth: usize, fair_scheduling: bool) -> QueueConfig {
        QueueConfig {
            max_depth,
            fair_scheduling,
        }
    }
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            max_depth: default_max_depth(),
            fair_scheduling: default_fair_scheduling(),
        }
    }
}
