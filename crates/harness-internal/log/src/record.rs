//! The values the run log stores and returns.
//!
//! The values the Harness hands a recorder (`Record`, `RunMeta`,
//! `RunOutcome`, and their parts) are the runner's own, re-exported here,
//! so a log and the effect loop share one definition of each.

use std::fmt;

pub use harness_runner::{Record, RecordKind, RunId, RunMeta, RunOutcome};

/// A record's position in its run: the effect loop's order, assigned by
/// the log in call order, starting at `0` and strictly increasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Seq(u64);

impl Seq {
    /// Wraps a raw position.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw position.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Which of a run's records to read. The default reads them all.
///
/// Records come back in `seq` order, the loop's order. `last` keeps only
/// the final `n`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordFilter {
    /// Only records of this kind.
    pub kind: Option<RecordKind>,
    /// Only the final `n` records.
    pub last: Option<u32>,
}

/// One record as the log returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StoredRecord {
    /// The record's position in its run.
    pub seq: Seq,
    /// When the record was appended, UTC milliseconds since the Unix epoch.
    pub at: i64,
    /// The record itself.
    pub record: Record,
}

/// One run as the log returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunRow {
    /// The run's identity.
    pub id: RunId,
    /// What the Harness knew when the run began.
    pub meta: RunMeta,
    /// When the run ended, UTC milliseconds since the Unix epoch; `None`
    /// while the run is open.
    pub ended_at: Option<i64>,
    /// How the run ended; `None` while the run is open.
    pub outcome: Option<RunOutcome>,
}
