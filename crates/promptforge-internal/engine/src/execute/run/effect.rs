//! Effects as values: what the Engine asks the Harness to perform, and what
//! the Harness answers with.
//!
//! A leaf request a section VM yields - a model round, a bound tool call,
//! a store operation, a timer - is not performed where it is dispatched.
//! The arm builds an [`Effect`], a plain description of the work, and the
//! run returns it from `step` for the Harness to perform; the Harness's
//! [`EffectAnswer`] comes back through `resume` keyed by the effect's
//! [`EffectId`], and the scheduler applies it on the caller's thread,
//! emitting the round's events there. The Engine thus decides *what* to do
//! and *what it means*; the Harness performs it.
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
use promptforge_types::event::ReplyOrigin;
use promptforge_types::ids::RoundId;
use promptforge_types::tools::{OutputTrust, ToolError, ToolId, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Completion, CompletionError, CompletionResult, Message, ToolSchema};
use crate::model::{CompletionOptions, ModelBinding, Temperature};
use promptforge_vfs::{Access, VfsError};

use crate::execute::protocol::{VfsOp, VfsOutcome};

/// Run-wide handle of one in-flight effect: an opaque correlation key
/// between an issued [`Effect`] and its [`EffectAnswer`]. Allocated from a
/// run-wide counter; it need not reproduce across runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(pub(crate) u64);

impl EffectId {
    /// The raw handle, for the Harness to key its log or its task table by
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

/// One model round's identity on its [`Effect::Chat`]: the round's id and
/// the path that dispatched it.
///
/// The id numbers the run's rounds from 0 in dispatch order, a section's
/// chat rounds and its nested `models.infer` rounds alike, and the
/// thinking, reply, and tool-call events the round's answer reports hold
/// the same id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Round {
    /// The round's run-wide id.
    pub id: RoundId,
    /// The path that dispatched the round: [`ReplyOrigin::Chat`] for a
    /// section's `chat` round (the `models.loop` rounds a Host streams
    /// live deltas from), [`ReplyOrigin::Infer`] for a nested
    /// `models.infer`, where only the completed reply is consumed.
    pub origin: ReplyOrigin,
}

/// One piece of work the Engine asks the Harness to perform.
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
        /// The round's id and origin. The Harness forwards the round's
        /// live deltas to its delta hook only when the origin is
        /// [`ReplyOrigin::Chat`]. The record keeps the id and leaves out
        /// the origin, which changes no request body.
        round: Round,
    },
    /// One bound tool call: `tool` is the stable identity the performer
    /// resolves to an implementation (the Harness against its activated
    /// capabilities, the Engine's internal table against the run's
    /// catalog), `alias` the prompt-local name it was called by, and
    /// `origin` who made the call and where, both kept for the record.
    ToolCall {
        /// The tool's stable live identity.
        tool: ToolId,
        /// The prompt-local alias the call named.
        alias: String,
        /// The call's arguments.
        args: Value,
        /// The run, the section, and the kind of caller that made the
        /// call.
        origin: ToolCallOrigin,
    },
    /// One operation on the run's store view - one of the eight `store.*`
    /// calls a prompt makes - under an ordinary access the Engine derived
    /// from the chain's capability at dispatch, rooted at the handle's
    /// declared store. Other code that touches the VFS, such as a tool or
    /// the Host reading files, does not appear as this effect. The
    /// Harness, performing the effect, uses the access exactly as given
    /// and within the scope it carries. When it drops never affects
    /// correctness: claims follow happens-before within the run's scope,
    /// which the run ends at `Done` or when it is dropped, after which the
    /// view refuses every operation.
    Vfs {
        /// The chain's store view: the chain's identity over the store
        /// root alone.
        access: Arc<Access>,
        /// The validated operation.
        op: VfsOp,
    },
    /// One sleep of `seconds`: the internal timeout behind a timed wait.
    Timer {
        /// The duration in seconds, non-negative and finite.
        seconds: f64,
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
                round,
                ..
            } => {
                let invocation = binding.invocation();
                EffectRecord::Chat {
                    round: round.id,
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
            Effect::ToolCall {
                tool,
                alias,
                args,
                origin,
            } => EffectRecord::ToolCall {
                tool: tool.clone(),
                alias: alias.clone(),
                args: args.clone(),
                origin: origin.clone(),
            },
            Effect::Vfs { op, .. } => EffectRecord::Vfs { op: op.clone() },
            Effect::Timer { seconds } => EffectRecord::Timer { seconds: *seconds },
        }
    }
}

/// An [`Effect`] minus its live handles: what a run log stores for the
/// effect and what a replay compares a re-issued effect against.
///
/// The `Chat` record flattens the round to what identifies it - its id,
/// the alias of the slot it ran under, and the frozen invocation - and
/// stores the messages in their wire form. It names the slot by alias
/// rather than by the bound model, because a Host's broker may serve the
/// slot with another model; the answer's [`ChatAnswerRecord`] names the
/// model that served the round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EffectRecord {
    /// One model round.
    Chat {
        /// The round's run-wide id, the one its content events hold.
        round: RoundId,
        /// The prompt-local alias of the slot the round ran under.
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
        /// The run, the section, and the kind of caller that made the
        /// call.
        origin: ToolCallOrigin,
    },
    /// One operation on the run's store view.
    Vfs {
        /// The validated operation.
        op: VfsOp,
    },
    /// One sleep.
    Timer {
        /// The duration in seconds.
        seconds: f64,
    },
}

/// Who made one tool call and where: the run's execution, the section
/// whose Lua was running, and whether the section's script or a model
/// round asked for the call. A log attributes the call by it, and the Host
/// can apply different policy to the same tool depending on its caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallOrigin {
    /// The run's execution identifier.
    pub execution: String,
    /// The section that made the call.
    pub section: String,
    /// Which kind of code asked for the call.
    pub caller: ToolCaller,
}

/// Which kind of code asked for one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolCaller {
    /// The section's own Lua called the tool through `tools.call`.
    Script,
    /// A model round requested the call.
    Model,
}

/// What a performer answers one [`Effect`] with: one variant per effect
/// kind, plus [`Dropped`](EffectAnswer::Dropped) for an effect the Harness
/// gave up on. Every effect receives exactly one answer.
#[derive(Debug)]
pub enum EffectAnswer {
    /// The model round's completion or its failure. Boxed: a completion
    /// holds both request and response bodies, and the box keeps every
    /// other answer's size from being set by this one.
    Chat(std::result::Result<Box<Completion>, CompletionError>),
    /// The tool's own output or its own failure, before the Engine's
    /// trust and count rules apply.
    ToolCall(std::result::Result<ToolOutput, ToolError>),
    /// The outcome of the operation on the run's store view, or the
    /// store's own structured failure.
    Vfs(std::result::Result<VfsOutcome, VfsError>),
    /// The timer fired.
    Timer,
    /// The Harness dropped the effect without performing it (a cancelled
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
            EffectAnswer::Vfs(result) => AnswerRecord::Vfs(match result {
                Ok(outcome) => Ok(outcome.clone()),
                Err(error) => Err(error.to_string()),
            }),
            EffectAnswer::Timer => AnswerRecord::Timer,
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
    /// The outcome of the operation on the run's store view, the
    /// [`VfsOutcome`] itself as the success payload, or its failure's
    /// display text.
    Vfs(std::result::Result<VfsOutcome, String>),
    /// The timer fired.
    Timer,
    /// The Harness dropped the effect without performing it.
    Dropped,
}

/// A completed model round as the log records it: what identifies the
/// answer without the request and response bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatAnswerRecord {
    /// The model that served the round, as the completion names it.
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
    /// The output text, before the Engine's trust rules apply.
    pub text: String,
    /// Whether the tool declared its output trusted.
    pub trusted: bool,
}

#[cfg(test)]
#[path = "effect-tests.rs"]
mod tests;
