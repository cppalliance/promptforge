//! Effects as values: what the engine asks a host to perform, and what the
//! host answers with.
//!
//! A leaf request a section VM yields - a model round, a bound tool call,
//! a wait for operator input, a store operation, a timer - is no longer
//! performed where it is dispatched. The arm builds an [`Effect`], a plain
//! description of the work, and the scheduler hands it to a performer; the
//! performer's [`EffectAnswer`] comes back keyed by the effect's
//! [`EffectId`], and the scheduler applies it on its own thread, emitting
//! the round's events there. The engine thus decides *what* to do and
//! *what it means*; performing is somebody else's job.
//!
//! An [`Effect`] may hold a live handle (the store access capability) and
//! so does not serialize itself. [`Effect::record`] projects it onto an
//! [`EffectRecord`], the effect minus its handles, which round-trips
//! through serde: a run log stores records, and a later replay compares a
//! re-executed run's records against them.

use std::sync::Arc;

use promptforge_api_types::tools::{ToolError, ToolId, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Completion, CompletionError, Message, ToolSchema};
use crate::input::{InputError, InputOutcome};
use crate::model::{CompletionOptions, ModelBinding, Temperature};
use crate::store::{Access, StoreError};

use super::protocol::{StoreOp, StoreOutcome};

/// Run-wide handle of one in-flight effect: an opaque correlation key
/// between an issued [`Effect`] and its [`EffectAnswer`]. Allocated from a
/// run-wide counter; it need not reproduce across runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EffectId(pub(crate) u64);

/// One piece of work the engine asks its host to perform.
#[derive(Debug)]
pub(crate) enum Effect {
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
        /// Whether the performer forwards the round's live deltas to the
        /// host's delta hook: `true` for a section's `chat` round (the
        /// `models.loop` rounds the hook is documented for), `false` for
        /// a nested `models.infer`, whose deltas have no consumer - only
        /// the completed reply is. Not part of the record: a delta is not
        /// an event, and the hint changes no request body.
        stream: bool,
    },
    /// One bound tool call: `tool` is the stable identity the performer
    /// resolves to an implementation (a host against its activated
    /// capabilities, the engine's internal table against the run's
    /// catalog), `alias` the prompt-local name it was called by, carried
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the engine issues effects and never reads them back; the run log is the record's first production reader"
        )
    )]
    pub(crate) fn record(&self) -> EffectRecord {
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
                    tools: tools.iter().map(|schema| schema.name.clone()).collect(),
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
        }
    }
}

/// An [`Effect`] minus its live handles: what a run log stores for the
/// effect and what a replay compares a re-issued effect against.
///
/// The `Chat` record flattens the binding to what identifies the round -
/// the model, the alias, and the frozen invocation - and carries the
/// messages in their wire form, so the record reads the same as the
/// request body the host would build from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum EffectRecord {
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
}

/// What a performer answers one [`Effect`] with: one variant per effect
/// kind, plus [`Dropped`](EffectAnswer::Dropped) for an effect the host
/// gave up on. Every effect receives exactly one answer.
#[derive(Debug)]
pub(crate) enum EffectAnswer {
    /// The model round's completion or its failure. Boxed: a completion
    /// carries both request and response bodies, and the box keeps every
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
    /// The host dropped the effect without performing it (a cancelled
    /// run): the chain resumes with a cancelled error.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "issued by the host's cancel path once `Run::resume` lands; the engine already applies it"
        )
    )]
    Dropped,
}

#[cfg(test)]
#[path = "run-tests.rs"]
mod tests;
