//! Hierarchical, deterministic identity for the engine's chains and tasks.
//!
//! A run holds no run-global id counter. Every chain (the main walk, a
//! `call` child, a spawned task) is named by a path: its parent chain's id
//! extended by the parent's local child counter, which `call` children and
//! spawned tasks share. The main walk is the root chain `0`. A task's id is
//! its chain's id. A section entry's id (`sys.id` in Lua) is its chain's
//! id extended by the chain's local entry counter. Two runs of the same
//! prompt with the same inputs allocate the same ids regardless of how
//! their chains interleave, because every counter is local to the chain
//! that advances it.
//!
//! The encoding is a dot-separated path of decimal components (`0`,
//! `0.2`, `0.2.0`), chosen over a packed integer because the depth and the
//! width of a run are both unbounded (call nesting, fanout arm count) and
//! because the path reads as the hierarchy it names in a log or a UI.
//!
//! [`TaskOrigin`] names the principal that started a task - the prompt's
//! author through `tasks.spawn`, or the model through its `task` tool -
//! and rides beside the task's id wherever the task is reported.
//! [`AbandonReason`] names how a task's owner ended while the task was
//! still live, for the `abandoned` terminal state. [`Provenance`] extends a
//! task's id with a per-task sequence number: the replay key stamped on
//! every effect and event the engine emits.
//!
//! Ids order as paths: a chain before its descendants, siblings by index.
//! The tasks one chain owns are its direct children, so sorting their ids
//! recovers spawn order.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[cfg(test)]
#[path = "ids-tests.rs"]
mod tests;

/// The hierarchical id of one chain: a path of child indices from the root
/// chain. Orders lexicographically as a path: a chain before its
/// descendants, siblings by child index.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChainId(Vec<u32>);

impl ChainId {
    /// The root chain: the main walk, whose id is `0`.
    #[must_use]
    pub fn root() -> Self {
        Self(vec![0])
    }

    /// The id of this chain's `index`-th child chain (a `call` child or a
    /// spawned task; the two share the parent's counter).
    #[must_use]
    pub fn child(&self, index: u32) -> Self {
        let mut components = Vec::with_capacity(self.0.len() + 1);
        components.extend_from_slice(&self.0);
        components.push(index);
        Self(components)
    }

    /// The id of this chain's `index`-th section entry, rendered as a
    /// path: the value a section reads as `sys.id`. A section id is not a
    /// chain id, so it is returned as text rather than as `ChainId`.
    #[must_use]
    pub fn entry(&self, index: u32) -> String {
        format!("{self}.{index}")
    }
}

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (position, component) in self.0.iter().enumerate() {
            if position > 0 {
                f.write_str(".")?;
            }
            write!(f, "{component}")?;
        }
        Ok(())
    }
}

/// The parse failure of a [`ChainId`] or [`TaskId`] path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid chain id `{input}`: required a dot-separated path of decimal components")]
pub struct ParseIdError {
    /// The rejected text.
    input: String,
}

impl ParseIdError {
    /// The rejected text.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }
}

impl FromStr for ChainId {
    type Err = ParseIdError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let reject = || ParseIdError {
            input: input.to_owned(),
        };
        if input.is_empty() {
            return Err(reject());
        }
        input
            .split('.')
            .map(|component| {
                // A component is plain decimal digits: no sign, no blank,
                // no leading `+`, which `u32::from_str` would otherwise
                // accept.
                if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(reject());
                }
                component.parse::<u32>().map_err(|_| reject())
            })
            .collect::<Result<Vec<u32>, ParseIdError>>()
            .map(Self)
    }
}

impl Serialize for ChainId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ChainId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The id of one task: its chain's id. A task and the chain that runs it
/// are one thing named from two sides, so the two ids are the same path;
/// the newtype keeps a task-keyed table from accepting an arbitrary chain
/// by accident. Orders as its chain id does.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(ChainId);

impl From<ChainId> for TaskId {
    fn from(chain: ChainId) -> Self {
        Self(chain)
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for TaskId {
    type Err = ParseIdError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        input.parse().map(Self)
    }
}

impl Serialize for TaskId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        ChainId::deserialize(deserializer).map(Self)
    }
}

/// The replay key of one effect or event: the nearest enclosing task and
/// the effect's or event's position within that task.
///
/// `task` is the task whose chain emitted the item. The main walk is task
/// `0`; a `call` child reports its parent's task, which is unambiguous
/// because a `call` blocks its parent, so the two never interleave. `seq`
/// is a counter local to that task, shared by its effects and its events
/// so the two kinds order against each other within one task. Two runs of
/// the same prompt with the same inputs and answers stamp the same
/// provenance on the same items regardless of how their chains interleave,
/// which is what lets a log slice by task, order within a task, and later
/// replay a run against its record: in durable-execution vocabulary this is
/// the replay key. The in-flight `EffectId` is a separate, opaque run-wide
/// handle that need not reproduce.
///
/// The name is chosen over `Origin` because [`shared_vfs::Origin`] already
/// names the claims origin label one crate below and [`TaskOrigin`] names
/// the spawning principal.
///
/// Orders by task path, then by sequence.
///
/// # Examples
/// ```
/// use promptforge_api_types::ids::{Provenance, TaskId};
///
/// let task: TaskId = "0.2".parse()?;
/// let first = Provenance { task: task.clone(), seq: 0 };
/// let second = Provenance { task, seq: 1 };
/// assert!(first < second);
/// # Ok::<(), promptforge_api_types::ids::ParseIdError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Provenance {
    /// The nearest enclosing task.
    pub task: TaskId,
    /// The item's position among the task's effects and events.
    pub seq: u32,
}

/// The principal that started a task.
///
/// The two are treated differently at the owner's chain end: an author
/// task that outlives its owner is the author's bug and fails the chain,
/// a model task that outlives its owner is abandoned and reported. The
/// tag is the string the Lua shims and the `tasks.pending` filter use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TaskOrigin {
    /// The prompt's author, through `tasks.spawn` (and `fanout` over it).
    Author,
    /// The model, through its `task` tool.
    Model,
}

impl TaskOrigin {
    /// The tag the shims and filters use: `author` or `model`.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            TaskOrigin::Author => "author",
            TaskOrigin::Model => "model",
        }
    }

    /// Parses a tag; `None` for anything outside the two exact tags.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "author" => Some(TaskOrigin::Author),
            "model" => Some(TaskOrigin::Model),
            _ => None,
        }
    }
}

/// Why a live task was abandoned: how its owner chain ended while the task
/// was still running.
///
/// A task ends with its owner. `abandoned` is kept apart from `cancelled`
/// because "lost its owner" and "was stopped on purpose" are different
/// facts for the log, the UI, and the model notice; the reason says which
/// kind of owner end it was, so the notice can say more than "abandoned".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AbandonReason {
    /// The owner ended normally - a scalar return or an exhausted walk -
    /// without waiting on or cancelling the task. For an author task this
    /// is the `tasks_live` error; a model task is abandoned quietly.
    OwnerReturned,
    /// The owner failed.
    OwnerFailed,
    /// The owner's model-tool loop ran past its round cap: a failure kept
    /// apart from [`OwnerFailed`](Self::OwnerFailed) because the model
    /// notice must say so - the model's own task outlived the loop that
    /// started it.
    ToolLoopExhausted,
    /// The owner was aborted from outside: a fatal sibling's fail-fast,
    /// or its own owner ending first.
    OwnerAborted,
    /// The run itself ended - cancelled by the host or ended by a fatal
    /// answer - while the task was live; the engine ended it with the run.
    RunTerminated,
}

impl AbandonReason {
    /// The phrase the `TaskAbandoned` trace line renders for the reason:
    /// `the section ended`, `the owner failed`, `the tool loop was
    /// exhausted`, `the owner was aborted`, or `the run ended`.
    #[must_use]
    pub fn why(self) -> &'static str {
        match self {
            AbandonReason::OwnerReturned => "the section ended",
            AbandonReason::OwnerFailed => "the owner failed",
            AbandonReason::ToolLoopExhausted => "the tool loop was exhausted",
            AbandonReason::OwnerAborted => "the owner was aborted",
            AbandonReason::RunTerminated => "the run ended",
        }
    }
}
