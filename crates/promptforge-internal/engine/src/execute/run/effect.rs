//! Effects as values: what the Engine asks its caller to perform, and what
//! the caller answers with.
//!
//! A leaf request a section VM yields - a model round, a bound tool call,
//! a store operation, a timer - is not performed where it is dispatched.
//! The arm builds an [`Effect`], a plain description of the work, and the
//! run returns it from `step` for the caller to perform; the caller's
//! [`EffectAnswer`] comes back through `resume` keyed by the effect's
//! [`EffectId`], and the scheduler applies it on the caller's thread,
//! emitting the round's events there. The Engine thus decides *what* to do
//! and *what it means*; the Harness performs it.
//!
//! An [`Effect`] may hold a live handle (a filesystem access) and so does
//! not serialize itself. [`Effect::record`] projects it onto an
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
pub use promptforge_types::tools::{ToolCallOrigin, ToolCaller};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Completion, CompletionError, CompletionResult, Message, ToolSchema};
use crate::model::{CompletionOptions, ModelBinding, Temperature};
use promptforge_vfs::{Access, VfsError};

use crate::execute::protocol::{VfsOp, VfsOutcome};

/// The identifier of one in-flight effect, which pairs an issued
/// [`Effect`] with its [`EffectAnswer`].
///
/// The id is opaque. The run allocates it from a run-wide counter, so the
/// same effect may get a different id in another run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(pub(crate) u64);

impl EffectId {
    /// Returns the id as a raw number. The caller can use it as a key, for
    /// example in its log or its task table. The number is meaningful only
    /// within the run that issued it.
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

/// The identity of one model round: its id and the path that dispatched
/// it. Every [`Effect::Chat`] carries one.
///
/// Ids number the run's model rounds from 0 in dispatch order. Chat rounds
/// and nested `models.infer` rounds share one sequence. The thinking,
/// reply, and tool-call events reported from the round's answer carry the
/// same id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Round {
    /// The round's run-wide id.
    pub id: RoundId,
    /// The path that dispatched the round. [`ReplyOrigin::Chat`] marks a
    /// section's `chat` round, the kind `models.loop` runs, whose live
    /// deltas the caller can stream. [`ReplyOrigin::Infer`] marks a nested
    /// `models.infer` round, where only the completed reply is consumed.
    pub origin: ReplyOrigin,
}

/// One piece of work the Engine asks the caller to perform.
#[derive(Debug)]
pub enum Effect {
    /// One model round: send `messages` to the model with `tools`
    /// advertised, using the frozen `options` built from `binding`. A
    /// nested `models.infer` round sends one user message with an empty
    /// tool list, and the Engine consumes only its completed reply.
    Chat {
        /// The binding the round runs under.
        binding: ModelBinding,
        /// The conversation to send, in wire order.
        messages: Vec<Message>,
        /// The earlier round whose request this one extends: the previous
        /// send of the same `messages.new()` list. `None` on a list's first
        /// send and on a nested `models.infer` round.
        after: Option<RoundId>,
        /// How many leading messages this request shares with `after`'s
        /// request; 0 when `after` is `None`.
        keep: u64,
        /// The tool schemas advertised to the model for this round.
        tools: Vec<ToolSchema>,
        /// The per-request completion options, built from `binding`.
        options: CompletionOptions,
        /// The round's id and origin. The caller needs to forward live
        /// deltas only for a round whose origin is [`ReplyOrigin::Chat`].
        /// The effect's record keeps the id and drops the origin, because
        /// the request sent to the model is the same for either origin.
        round: Round,
    },
    /// One call to a bound tool. The caller resolves `tool`, the tool's
    /// stable identity, to an implementation. `alias` is the prompt-local
    /// name the call used, and `origin` says who made the call and where.
    /// The effect's record keeps both and leaves out `access`.
    ToolCall {
        /// The tool's stable live identity.
        tool: ToolId,
        /// The prompt-local alias the call used.
        alias: String,
        /// The call's arguments.
        args: Value,
        /// Who made the call: the run, the section, and whether the
        /// section's script or a model round asked for it.
        origin: ToolCallOrigin,
        /// The call's own identity, forked from the calling chain's
        /// access when the call is issued and joined back into it when the
        /// answer is applied or the call is aborted, so the chain's earlier
        /// work happens before the tool's and the tool's before the chain's
        /// next step. It is rooted at `/`. Once the answer is applied or the
        /// call is aborted, every operation through it is refused.
        access: Arc<Access>,
    },
    /// One operation on the run's store view, issued for one of the eight
    /// `store.*` calls a prompt can make.
    ///
    /// `access` is an ordinary access that the Engine derives from the
    /// chain's capability at dispatch. It is rooted at the store declared
    /// by the handle that the chain's access came from. Only the `store.*`
    /// calls produce this effect. A tool reaches files through the access
    /// its [`ToolCall`](Effect::ToolCall) effect carries.
    ///
    /// The caller must use the access exactly as given and within the
    /// scope it carries. Correctness holds whenever the caller drops the
    /// access. Store claims follow happens-before order within the
    /// run's scope. The run ends that scope at `Done` or when the run is
    /// dropped, and after that the view refuses every operation.
    Vfs {
        /// The chain's store view: an access with the chain's identity
        /// that reaches only the store root.
        access: Arc<Access>,
        /// The validated operation.
        op: VfsOp,
    },
    /// One sleep of `seconds` seconds, which the Engine uses as the
    /// timeout behind a timed wait.
    Timer {
        /// The duration in seconds, finite and zero or greater.
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
    /// Returns the effect's record: the same request with its live handles
    /// removed, in a form that a run log stores and a replay compares.
    #[must_use]
    pub fn record(&self) -> EffectRecord {
        match self {
            Effect::Chat {
                binding,
                messages,
                after,
                keep,
                tools,
                round,
                ..
            } => {
                let invocation = binding.invocation();
                let kept = usize::try_from(*keep).unwrap_or(usize::MAX);
                EffectRecord::Chat {
                    round: round.id,
                    alias: binding.alias().to_owned(),
                    after: *after,
                    keep: *keep,
                    messages: messages.iter().skip(kept).map(wire_value).collect(),
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
                ..
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

/// An [`Effect`] with its live handles removed, as a run log stores it. A
/// replay compares each re-issued effect against its record.
///
/// The `Chat` record keeps only what identifies the round: its id, the
/// alias of the model slot it ran under, and the frozen invocation
/// settings (temperature, generation cap, and thinking switch). It stores
/// the request's messages in wire form after the first `keep`, which repeat
/// round `after`'s request. It names the slot by alias, because the
/// caller may serve the slot with a different model from the bound one.
/// The answer's [`ChatAnswerRecord`] names the model that served the round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EffectRecord {
    /// One model round.
    Chat {
        /// The round's run-wide id, which the round's content events also
        /// carry.
        round: RoundId,
        /// The prompt-local alias of the slot the round ran under.
        alias: String,
        /// The earlier round whose request this one extends, or `None`
        /// when the request starts here.
        after: Option<RoundId>,
        /// How many leading messages of `after`'s request this request
        /// repeats; 0 when `after` is `None`.
        keep: u64,
        /// The request's messages after the first `keep`, one wire-form
        /// message per entry.
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
        /// The prompt-local alias the call used.
        alias: String,
        /// The call's arguments.
        args: Value,
        /// Who made the call: the run, the section, and whether the
        /// section's script or a model round asked for it.
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

/// The answer to one [`Effect`]: one variant per effect kind, plus
/// [`Dropped`](EffectAnswer::Dropped) for an effect the caller gave up on.
/// Every effect receives exactly one answer.
#[derive(Debug)]
pub enum EffectAnswer {
    /// The model round's completion or its failure.
    Chat(std::result::Result<Box<Completion>, CompletionError>),
    /// The tool's own output or its own failure, before the Engine applies
    /// its trust rule.
    ToolCall(std::result::Result<ToolOutput, ToolError>),
    /// The outcome of the operation on the run's store view, or the
    /// store's own structured failure.
    Vfs(std::result::Result<VfsOutcome, VfsError>),
    /// The timer fired.
    Timer,
    /// The caller gave up on the effect and dropped it, for example
    /// because the run was cancelled or the effect's task ended first. If
    /// the chain still waits on the effect, it resumes with a cancelled
    /// error. A drop is an answer like any other, so it counts as the
    /// effect's one answer.
    Dropped,
}

impl EffectAnswer {
    /// Returns the answer's record: the parts of its outcome that a log can
    /// hold whole. A failure is recorded as its display text. A completion
    /// is recorded as its reply or its requested tool names. The round's
    /// request and response bodies travel as debug events, and its metrics
    /// travel in the turn's event.
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
    /// The outcome of the operation on the run's store view: the
    /// [`VfsOutcome`] itself on success, or the failure's display text.
    Vfs(std::result::Result<VfsOutcome, String>),
    /// The timer fired.
    Timer,
    /// The caller gave up on the effect and dropped it.
    Dropped,
}

/// A completed model round as the log records it: what identifies the
/// answer. The round's request and response bodies travel separately, as
/// debug events.
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
