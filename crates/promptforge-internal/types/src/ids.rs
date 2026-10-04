//! Hierarchical, deterministic identity for the Engine's chains and tasks.
//!
//! Every chain (the main walk, a `call` child, a spawned task) is named by
//! a path: its parent chain's id extended by the parent's local child
//! counter, which `call` children and spawned tasks share. The main walk
//! is the root chain `0`. A task's id is its chain's id. A section entry's
//! id (`sys.id` in Lua) is its chain's id extended by the chain's local
//! entry counter. Two runs of the same prompt with the same inputs
//! allocate the same ids regardless of how their chains interleave,
//! because every counter is local to the chain that advances it.
//!
//! The encoding is a dot-separated path of decimal components (`0`,
//! `0.2`, `0.2.0`), chosen over a packed integer because the depth and the
//! width of a run are both unbounded (call nesting, fanout arm count) and
//! because the path reads as the hierarchy it names in a log or a UI.
//!
//! [`TaskOrigin`] names the principal that started a task - the prompt's
//! author through `tasks.spawn`, or the model through its `task` tool -
//! and appears beside the task's id wherever the task is reported.
//! [`AbandonReason`] names how a task's owner ended while the task was
//! still live, for the `abandoned` terminal state. [`Provenance`] extends a
//! task's id with a per-task sequence number: the replay key stamped on
//! every effect and event the Engine emits. [`RoundId`] numbers the run's
//! model rounds, so a round's effect and its content events name it alike.
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
/// chain.
///
/// It displays and serializes as decimal components joined by dots, such
/// as `0.2.0`. Ids order as paths: a chain before its descendants, and
/// siblings by child index.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChainId(Vec<u32>);

impl ChainId {
    /// The root chain: the main walk, whose id is `0`.
    #[must_use]
    pub fn root() -> Self {
        Self(vec![0])
    }

    /// The id of this chain's child chain number `index`. A `call` child
    /// and a spawned task are both child chains, and they share the
    /// parent's counter.
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

/// The error returned when text is not a valid [`ChainId`] or [`TaskId`]
/// path.
///
/// A valid path is one or more components joined by dots, such as `0.2.0`.
/// Each component is plain decimal digits that fit in a `u32`.
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

/// The id of one task, which is the id of the chain that runs it.
///
/// A task and its chain are one thing named from two sides, so both ids are
/// the same path. The separate type keeps a table keyed by task from
/// accepting an arbitrary chain id by mistake. Orders as its chain id does.
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

/// The replay key of one effect or event: its nearest enclosing task and its
/// position within that task.
///
/// `task` is the task whose chain emitted the item. The main walk is task
/// `0`. A `call` child reports its parent's task. This is unambiguous
/// because a `call` blocks its parent, so the two never interleave. `seq`
/// is a counter local to that task. The task's effects and events share
/// it, so the two kinds order against each other within one task.
///
/// Two runs of the same prompt with the same inputs and answers stamp the
/// same provenance on the same items, regardless of how their chains
/// interleave. That lets a log slice by task, order items within a task,
/// and later replay a run against its record. The `EffectId` of an
/// in-flight effect is a separate, opaque handle counted across the whole
/// run, and it can differ between runs.
///
/// Orders by task path, then by sequence.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Provenance {
    /// The nearest enclosing task.
    pub task: TaskId,
    /// The item's position among the task's effects and events.
    pub seq: u32,
}

/// The id of one model round: its position in the run's dispatch order,
/// counting from 0.
///
/// One counter for the whole run numbers every round, both a section's chat
/// rounds and its nested `models.infer` rounds. A round's `Chat` effect
/// holds its id, and so do the thinking, reply, and tool-call events
/// reported from the round's answer. The caller can use the id to pair the
/// partial output it shows while a round runs with the events that report
/// the round's answer. Like an effect id, it counts across the whole run.
/// When tasks run concurrently, the order in which the caller answers
/// effects can change which round gets which number. Serializes as a bare
/// number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoundId(u64);

impl RoundId {
    /// The round numbered `id`.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The round's number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The principal that started a task: the prompt's author or the model.
///
/// The two are treated differently when the owner chain ends while the
/// task is still live. An author task that outlives its owner is the
/// author's bug, so it fails the owner chain. A model task that outlives
/// its owner is abandoned and reported. Lua code writes an origin as its
/// tag, `author` or `model`, for example in the `origin` filter of
/// `tasks.pending`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TaskOrigin {
    /// The prompt's author, through `tasks.spawn` or `fanout`, which is
    /// built on it.
    Author,
    /// The model, through its `task` tool.
    Model,
}

impl TaskOrigin {
    /// The origin's tag as Lua code writes it: `author` or `model`.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            TaskOrigin::Author => "author",
            TaskOrigin::Model => "model",
        }
    }

    /// Parses a tag. Returns `None` for anything other than exactly
    /// `author` or `model`.
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
/// A task ends with its owner. The `abandoned` terminal state is separate
/// from `cancelled`, because losing an owner and being stopped on purpose
/// are different facts for a log, a UI, and the notice the model receives.
/// The reason says which kind of owner end it was, so the notice can say
/// more than "abandoned".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AbandonReason {
    /// The owner ended normally, by returning a scalar or finishing its
    /// walk, without waiting on or cancelling the task. For an author task,
    /// the owner's outcome becomes the `tasks_live` error. A model task is
    /// abandoned without an error.
    OwnerReturned,
    /// The owner failed.
    OwnerFailed,
    /// The owner's model tool loop ran past its round cap. This is a
    /// failure, reported apart from [`OwnerFailed`](Self::OwnerFailed)
    /// because the notice the model receives must say that the model's own
    /// task outlived the loop that started it.
    ToolLoopExhausted,
    /// The owner was aborted from outside, either by fail-fast after a
    /// sibling failed fatally or because the owner's own owner ended first.
    OwnerAborted,
    /// The run itself ended while the task was live, because the caller
    /// cancelled the run or an answer was fatal. The Engine ended the task
    /// with the run.
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
