//! The recorder seam: where the Harness writes a run's history, and the
//! values it writes.
//!
//! The Harness hands a [`RunRecorder`] each run's start, then every
//! effect, answer, and event in effect-loop order, then its end. The
//! Host supplies the recorder, so the Harness holds no database and no
//! storage path. [`MemoryRecorder`] keeps runs in memory for tests and
//! simple Hosts.
//!
//! The Harness awaits each call before it goes on, so a step's events
//! reach the recorder before the step's effects start. Calls within one
//! run never overlap. Calls from different runs may, so an implementation
//! serializes its own state. A recorder that fails stops its run: the
//! Harness never skips a record. A Host that prefers to keep running logs
//! the failure inside its recorder and returns `Ok`.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

#[path = "recorder-memory.rs"]
mod memory;

pub use memory::MemoryRecorder;

/// A boxed, sendable future that borrows the recorder: what each
/// [`RunRecorder`] call returns.
///
/// The name keeps it apart from the effect loop's per-event `sink`
/// callback.
pub type RecorderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, RecorderError>> + Send + 'a>>;

/// Why a recorder could not take a write; the cause is the source.
///
/// The Harness stops the run that hit it. The text names the failing
/// layer only, so read the cause chain for the reason.
#[derive(Debug, thiserror::Error)]
#[error("the run recorder failed")]
pub struct RecorderError {
    /// The recorder's own error.
    #[source]
    source: Box<dyn Error + Send + Sync>,
}

impl RecorderError {
    /// Wraps the error, or the message, that made the recorder fail.
    #[must_use]
    pub fn new(source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

/// Where the Harness writes a run's history.
///
/// A run calls [`begin_run`](Self::begin_run) once, then
/// [`append`](Self::append) for every record, then
/// [`end_run`](Self::end_run) once. The recorder issues each [`RunId`].
/// A write that returns an error ends the run it belongs to.
pub trait RunRecorder: Send + Sync {
    /// Opens a run's history and returns its id.
    ///
    /// # Errors
    ///
    /// Returns the recorder's own failure, wrapped in a [`RecorderError`].
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId>;

    /// Adds one record to an open run.
    ///
    /// # Errors
    ///
    /// Returns the recorder's own failure, wrapped in a [`RecorderError`].
    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()>;

    /// Closes a run with its outcome. The Harness calls it once for each
    /// run it began.
    ///
    /// # Errors
    ///
    /// Returns the recorder's own failure, wrapped in a [`RecorderError`].
    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()>;
}

/// A run's identity as its recorder issued it: whatever
/// [`RunRecorder::begin_run`] returned. It means something only to the
/// recorder that issued it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(i64);

impl RunId {
    /// Wraps a raw id, for callers that stored one.
    #[must_use]
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    /// The raw id.
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

/// What the Harness knows about a run when it begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMeta {
    /// The session that launched the run.
    pub session_id: String,
    /// The agent the session runs.
    pub agent: String,
    /// A content hash of the prompt file, so a transcript can be matched
    /// to the exact text that produced it.
    pub prompt_hash: String,
    /// The Harness-drawn seed handed to the Engine.
    pub seed: u64,
    /// The Engine's behavior flags, a bitset; empty until a flag exists.
    pub flags: u32,
    /// When the run started, UTC milliseconds since the Unix epoch; the
    /// Engine's `started_at` input, so the record and the run agree.
    pub started_at: i64,
}

/// Which side of the effect loop a record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordKind {
    /// An effect the Engine issued; the payload is an `EffectRecord`.
    Effect,
    /// The answer to an effect; the payload is an `EffectAnswer`.
    Answer,
    /// An event the Engine emitted; the payload is an `Event`.
    Event,
}

impl RecordKind {
    /// The kind's stored text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Effect => "effect",
            Self::Answer => "answer",
            Self::Event => "event",
        }
    }

    /// Parses stored kind text; `None` for text no kind writes.
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

/// One record as the Harness appends it. The recorder decides each
/// record's position and time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The nearest enclosing task, as the Engine's `TaskId` renders: a
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

/// How a run ended. Mirrors the Engine's `RunResult`.
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
    /// The Host cancelled the run.
    Cancelled,
}

impl RunOutcome {
    /// The outcome's stored text.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Completed { .. } => "completed",
            Self::Failed { .. } => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}
