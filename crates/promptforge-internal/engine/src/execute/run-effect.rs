//! Effects as values: what the engine asks a host to perform, and what the
//! host answers with.
//!
//! A leaf request a section VM yields - a model round, a bound tool call,
//! a wait for operator input, a store operation, a timer - is not
//! performed where it is dispatched. The arm builds an [`Effect`], a plain
//! description of the work, and the run returns it from `step` for the
//! host to perform; the host's [`EffectAnswer`] comes back through
//! `resume` keyed by the effect's [`EffectId`], and the scheduler applies
//! it on the caller's thread, emitting the round's events there. The
//! engine thus decides *what* to do and *what it means*; performing is the
//! host's job.
//!
//! An [`Effect`] may hold a live handle (the store access capability) and
//! so does not serialize itself. [`Effect::record`] projects it onto an
//! [`EffectRecord`], the effect minus its handles, which round-trips
//! through serde: a run log stores records, and a later replay compares a
//! re-executed run's records against them. An [`EffectAnswer`] likewise
//! contains values a log cannot hold whole (a completion's bodies, an
//! error's boxed cause); [`EffectAnswer::record`] projects it onto an
//! [`AnswerRecord`], the answer's outcome as a log stores it.

use std::sync::Arc;

use promptforge_model_client::detail::tool_schema_name;
use promptforge_types::event::Event;
use promptforge_types::ids::TaskId;
use promptforge_types::tools::{OutputTrust, ToolError, ToolId, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::input::{InputError, InputOutcome};
use crate::model::{Completion, CompletionError, CompletionResult, Message, ToolSchema};
use crate::model::{CompletionOptions, ModelBinding, Temperature};
use crate::store::{Access, StoreError};

use crate::execute::protocol::{StoreOp, StoreOutcome};

/// Run-wide handle of one in-flight effect: an opaque correlation key
/// between an issued [`Effect`] and its [`EffectAnswer`]. Allocated from a
/// run-wide counter; it need not reproduce across runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(pub(crate) u64);

impl EffectId {
    /// The raw handle, for a host that keys its log or its task table by
    /// it. Meaningful only within the run that issued it.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for EffectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One piece of work the engine asks its host to perform.
#[derive(Debug)]
pub enum Effect {
    /// One model round over `messages` with `tools` advertised, under
    /// `binding`'s frozen `options`. A nested `models.infer` is a round
    /// over one user message with no tools and no live deltas.
    Chat {
        /// The binding the round runs under.
        binding: ModelBinding,
        /// The projected conversation, in wire order.
        messages: Vec<Message>,
        /// The tool schemas advertised for the round; empty advertises
        /// none.
        tools: Vec<ToolSchema>,
        /// The per-request fields, built from `binding`.
        options: CompletionOptions,
        /// Whether the host forwards the round's live deltas to its delta
        /// hook: `true` for a section's `chat` round (the `models.loop`
        /// rounds the hook is documented for), `false` for a nested
        /// `models.infer`, where only the completed reply is consumed.
        /// Not part of the record: a delta is not an event, and the hint
        /// changes no request body.
        stream: bool,
    },
    /// One bound tool call: `tool` is the stable identity the performer
    /// resolves to an implementation (a host against its activated
    /// capabilities, the engine's internal table against the run's
    /// catalog), `alias` the prompt-local name it was called by, kept
    /// for the record.
    ToolCall {
        /// The tool's stable live identity.
        tool: ToolId,
        /// The prompt-local alias the call named.
        alias: String,
        /// The call's arguments.
        args: Value,
    },
    /// One wait for operator input, for `section` of `execution`.
    UserInput {
        /// The run's execution identifier.
        execution: String,
        /// The section asking.
        section: String,
    },
    /// One store operation under the chain's access capability. The
    /// handle is minted by the engine from the chain's claims; a host
    /// performing the effect uses it as given and never derives, widens,
    /// or retains store scope from it.
    Store {
        /// The chain's access capability, released when the operation
        /// completes.
        access: Arc<Access>,
        /// The validated operation.
        op: StoreOp,
    },
    /// One sleep of `seconds`: the internal timeout behind a timed wait.
    Timer {
        /// The duration in seconds, non-negative and finite.
        seconds: f64,
    },
    /// One read of a task's reported history: every event whose
    /// provenance names `task` with a sequence number after `last` (all of
    /// them when `last` is `None`), in sequence order. The host answers
    /// from its log - the events it was handed by earlier steps, which it
    /// commits before performing the step's effects, so a task reading its
    /// history sees everything reported before the read was issued.
    TaskEvents {
        /// The task whose events are read.
        task: TaskId,
        /// The highest sequence number the reader has already seen, when
        /// it has seen any.
        last: Option<u32>,
    },
}

/// One value's serde wire form. Every type recorded here serializes
/// infallibly (strings, numbers, and JSON values), so the `Null` fallback
/// is unreachable in practice and stands only so the projection stays
/// total.
fn wire_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

impl Effect {
    /// The effect's record: the same request minus its live handles, in a
    /// form a log stores and a replay compares.
    #[must_use]
    pub fn record(&self) -> EffectRecord {
        match self {
            Effect::Chat {
                binding,
                messages,
                tools,
                ..
            } => {
                let invocation = binding.invocation();
                EffectRecord::Chat {
                    model: binding.id().name().to_owned(),
                    alias: binding.alias().to_owned(),
                    messages: messages.iter().map(wire_value).collect(),
                    tools: tools
                        .iter()
                        .map(|schema| tool_schema_name(schema).to_owned())
                        .collect(),
                    temperature: invocation.temperature.map(Temperature::get),
                    max_tokens: invocation.max_tokens.map(std::num::NonZeroU32::get),
                    thinking: invocation.thinking,
                }
            }
            Effect::ToolCall { tool, alias, args } => EffectRecord::ToolCall {
                tool: tool.clone(),
                alias: alias.clone(),
                args: args.clone(),
            },
            Effect::UserInput { execution, section } => EffectRecord::UserInput {
                execution: execution.clone(),
                section: section.clone(),
            },
            Effect::Store { op, .. } => EffectRecord::Store { op: op.clone() },
            Effect::Timer { seconds } => EffectRecord::Timer { seconds: *seconds },
            Effect::TaskEvents { task, last } => EffectRecord::TaskEvents {
                task: task.clone(),
                last: *last,
            },
        }
    }
}

/// An [`Effect`] minus its live handles: what a run log stores for the
/// effect and what a replay compares a re-issued effect against.
///
/// The `Chat` record flattens the binding to what identifies the round -
/// the model, the alias, and the frozen invocation - and stores the
/// messages in their wire form, so the record reads the same as the
/// request body the host would build from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EffectRecord {
    /// One model round.
    Chat {
        /// The bound model's name.
        model: String,
        /// The prompt-local alias the round ran under.
        alias: String,
        /// The conversation, one wire-form message per entry.
        messages: Vec<Value>,
        /// The advertised tool names, in schema order.
        tools: Vec<String>,
        /// The frozen sampling temperature, when the bind declared one.
        temperature: Option<f64>,
        /// The frozen generation cap, when the bind declared one.
        max_tokens: Option<u32>,
        /// The frozen thinking switch, when the bind declared one.
        thinking: Option<bool>,
    },
    /// One bound tool call.
    ToolCall {
        /// The tool's stable live identity.
        tool: ToolId,
        /// The prompt-local alias the call named.
        alias: String,
        /// The call's arguments.
        args: Value,
    },
    /// One wait for operator input.
    UserInput {
        /// The run's execution identifier.
        execution: String,
        /// The section asking.
        section: String,
    },
    /// One store operation.
    Store {
        /// The validated operation.
        op: StoreOp,
    },
    /// One sleep.
    Timer {
        /// The duration in seconds.
        seconds: f64,
    },
    /// One read of a task's reported history.
    TaskEvents {
        /// The task whose events are read.
        task: TaskId,
        /// The highest sequence number the reader has already seen.
        last: Option<u32>,
    },
}

/// What a performer answers one [`Effect`] with: one variant per effect
/// kind, plus [`Dropped`](EffectAnswer::Dropped) for an effect the host
/// gave up on. Every effect receives exactly one answer.
#[derive(Debug)]
pub enum EffectAnswer {
    /// The model round's completion or its failure. Boxed: a completion
    /// holds both request and response bodies, and the box keeps every
    /// other answer's size from being set by this one.
    Chat(std::result::Result<Box<Completion>, CompletionError>),
    /// The tool's own output or its own failure, before the engine's
    /// trust and count rules apply.
    ToolCall(std::result::Result<ToolOutput, ToolError>),
    /// The broker's outcome or its failure.
    UserInput(std::result::Result<InputOutcome, InputError>),
    /// The store operation's outcome or the store's own failure.
    Store(std::result::Result<StoreOutcome, StoreError>),
    /// The timer fired.
    Timer,
    /// The task's events after the read's `last`, in sequence order, as
    /// the host's log holds them.
    TaskEvents(Vec<Event>),
    /// The host dropped the effect without performing it (a cancelled
    /// run, or an effect whose task ended first): the chain, if it still
    /// waits, resumes with a cancelled error. A drop is an answer like any
    /// other, so every issued effect receives exactly one.
    Dropped,
}

impl EffectAnswer {
    /// The answer's record: its outcome minus what a log cannot hold
    /// whole. A failure is recorded as its display text; a completion as
    /// the reply or the requested tool names, since the round's bodies
    /// travel as debug events and its metrics as the turn's event.
    #[must_use]
    pub fn record(&self) -> AnswerRecord {
        match self {
            EffectAnswer::Chat(result) => AnswerRecord::Chat(match result {
                Ok(completion) => Ok(ChatAnswerRecord::from(completion.as_ref())),
                Err(error) => Err(error.to_string()),
            }),
            EffectAnswer::ToolCall(result) => AnswerRecord::ToolCall(match result {
                Ok(output) => Ok(ToolAnswerRecord {
                    text: output.text().to_owned(),
                    trusted: output.trust() == OutputTrust::Trusted,
                }),
                Err(error) => Err(error.to_string()),
            }),
            EffectAnswer::UserInput(result) => AnswerRecord::UserInput(match result {
                Ok(InputOutcome::Text(text)) => Ok(InputAnswerRecord::Text(text.clone())),
                Ok(InputOutcome::Unavailable) => Ok(InputAnswerRecord::Unavailable),
                Err(error) => Err(error.to_string()),
            }),
            EffectAnswer::Store(result) => AnswerRecord::Store(match result {
                Ok(StoreOutcome::Unit) => Ok(StoreAnswerRecord::Unit),
                Ok(StoreOutcome::Text(text)) => Ok(StoreAnswerRecord::Text(text.clone())),
                Ok(StoreOutcome::Paths(paths)) => Ok(StoreAnswerRecord::Paths(paths.clone())),
                Ok(StoreOutcome::Bool(flag)) => Ok(StoreAnswerRecord::Bool(*flag)),
                Err(error) => Err(error.to_string()),
            }),
            EffectAnswer::Timer => AnswerRecord::Timer,
            EffectAnswer::TaskEvents(events) => AnswerRecord::TaskEvents(events.clone()),
            EffectAnswer::Dropped => AnswerRecord::Dropped,
        }
    }
}

/// An [`EffectAnswer`] as a run log stores it: one variant per answer
/// kind, each holding its outcome with every failure rendered to its
/// display text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnswerRecord {
    /// The model round's outcome.
    Chat(std::result::Result<ChatAnswerRecord, String>),
    /// The tool call's outcome.
    ToolCall(std::result::Result<ToolAnswerRecord, String>),
    /// The input wait's outcome.
    UserInput(std::result::Result<InputAnswerRecord, String>),
    /// The store operation's outcome.
    Store(std::result::Result<StoreAnswerRecord, String>),
    /// The timer fired.
    Timer,
    /// The task's events after the read's `last`, in sequence order.
    TaskEvents(Vec<Event>),
    /// The host dropped the effect without performing it.
    Dropped,
}

/// A completed model round as the log records it: what identifies the
/// answer without the request and response bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatAnswerRecord {
    /// The model that served the round, as the response body named it.
    pub model: String,
    /// The provider's finish reason, when it sent one.
    pub finish_reason: Option<String>,
    /// The reply text, when the round produced text.
    pub reply: Option<String>,
    /// The names of the tools the model requested, in call order, when it
    /// requested any.
    pub tool_calls: Vec<String>,
}

impl From<&Completion> for ChatAnswerRecord {
    fn from(completion: &Completion) -> Self {
        let (reply, tool_calls) = match completion.result() {
            CompletionResult::Text(text) => (Some(text.clone()), Vec::new()),
            CompletionResult::ToolCalls(calls) => (
                None,
                calls.iter().map(|call| call.name().to_owned()).collect(),
            ),
            // The vocabulary is `#[non_exhaustive]`; a variant this crate
            // does not know records as a round with neither product.
            _ => (None, Vec::new()),
        };
        ChatAnswerRecord {
            model: completion.model().to_owned(),
            finish_reason: completion.finish_reason().map(str::to_owned),
            reply,
            tool_calls,
        }
    }
}

/// A tool's own output as the log records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnswerRecord {
    /// The output text, before the engine's trust rules apply.
    pub text: String,
    /// Whether the tool declared its output trusted.
    pub trusted: bool,
}

/// An input wait's outcome as the log records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputAnswerRecord {
    /// The operator supplied text.
    Text(String),
    /// The host had no input to give.
    Unavailable,
}

/// A store operation's outcome as the log records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoreAnswerRecord {
    /// The operation succeeded with no return value.
    Unit,
    /// A read's text.
    Text(String),
    /// A glob's matching paths.
    Paths(Vec<String>),
    /// An existence check's flag.
    Bool(bool),
}

#[cfg(test)]
#[path = "run-effect-tests.rs"]
mod tests;
