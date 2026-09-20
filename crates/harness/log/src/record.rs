//! The values the run log stores and returns.

use std::fmt;

/// A run's identity within one log: the `runs` row id, allocated by the
/// database at `begin_run`. Meaningful only against the log that issued it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(i64);

impl RunId {
    /// Wraps a raw row id, for callers that stored one.
    #[must_use]
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    /// The raw row id.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

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

/// What the harness knows about a run when it begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMeta {
    /// The session that launched the run.
    pub session_id: String,
    /// The agent the session runs.
    pub agent: String,
    /// A content hash of the prompt file, so a transcript can be matched
    /// to the exact text that produced it.
    pub prompt_hash: String,
    /// The host-drawn seed handed to the engine.
    pub seed: u64,
    /// The engine's behavior flags, a bitset; empty until a flag exists.
    pub flags: u32,
    /// When the run started, UTC milliseconds since the Unix epoch; the
    /// engine's `started_at` input, so the log and the run agree.
    pub started_at: i64,
}

/// Which side of the effect loop a record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordKind {
    /// An effect the engine issued; the payload is an `EffectRecord`.
    Effect,
    /// The answer to an effect; the payload is an `EffectAnswer`.
    Answer,
    /// An event the engine emitted; the payload is an `Event`.
    Event,
}

impl RecordKind {
    /// The stored `kind` text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Effect => "effect",
            Self::Answer => "answer",
            Self::Event => "event",
        }
    }

    /// Parses stored `kind` text.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "effect" => Some(Self::Effect),
            "answer" => Some(Self::Answer),
            "event" => Some(Self::Event),
            _ => None,
        }
    }
}

/// One record as the harness appends it. `seq` and `at` are the log's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The nearest enclosing task, as the engine's `TaskId` renders: a
    /// dot-separated path of child indices from the root chain, so the
    /// main walk is task `0` and its second child task is `0.1`.
    pub task_id: String,
    /// The record's position within its task.
    pub task_seq: u32,
    /// Which side of the loop the record came from.
    pub kind: RecordKind,
    /// The in-flight effect handle, for effects and their answers.
    pub effect_id: Option<u64>,
    /// The serialized `EffectRecord`, `EffectAnswer`, or `Event`.
    pub payload: serde_json::Value,
}

/// Which of a run's records to read. The default reads them all.
///
/// Without `task`, records come back in `seq` order, the loop's order.
/// With `task`, they come back in that task's own `task_seq` order, which
/// can differ from `seq` when tasks interleave. `last` keeps only the
/// final `n` in whichever order applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordFilter {
    /// Only records of this kind.
    pub kind: Option<RecordKind>,
    /// Only records from this task (its rendered path), ordered by
    /// `task_seq`.
    pub task: Option<String>,
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

/// How a run ended. Mirrors the engine's `RunResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// The run completed with its final text.
    Completed {
        /// The run's final text.
        final_text: String,
    },
    /// The run failed.
    Failed {
        /// The failure's classification.
        kind: String,
        /// The failure's message.
        message: String,
    },
    /// The host cancelled the run.
    Cancelled,
}

impl RunOutcome {
    /// The stored `outcome` text.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One run as the log returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunRow {
    /// The run's identity.
    pub id: RunId,
    /// What the harness knew when the run began.
    pub meta: RunMeta,
    /// When the run ended, UTC milliseconds since the Unix epoch; `None`
    /// while the run is open.
    pub ended_at: Option<i64>,
    /// How the run ended; `None` while the run is open.
    pub outcome: Option<RunOutcome>,
}
