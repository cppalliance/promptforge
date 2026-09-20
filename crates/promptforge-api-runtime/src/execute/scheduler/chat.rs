//! The `chat` arm: one stateless tool-capable model round yielded by a
//! section VM.
//!
//! Dispatch resolves the round's binding and tool scope (the bound and
//! local halves, plus the model's task built-ins once the section has run
//! `tools.allow_tasks`), records the scope on the chain as `advertised`,
//! prechecks the projected conversation against the model's context
//! window, and issues the single gateway round as a `Chat` effect - the
//! same effect a nested `infer` issues, over the author's conversation
//! and the advertised schemas. The driver classifies the answered
//! completion into the round's answer when it arrives
//! ([`Scheduler::accept_chat`]), emitting the round's events - the turn
//! advance, the debug capture pair, turn completed or failed or truncated,
//! thinking, and the reply or the tool-call batch - through the chain's
//! own task-scoped emitter, and rejecting a tool name outside the scope
//! the chain advertised for that round. The shim that yielded the round
//! emits nothing; the scheduler owns every event.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use promptforge_api_types::metrics::{CallMetrics, ToolCallEvent};

use crate::execute::protocol::{Answer, ChatResult};
use crate::execute::run::Effect;
use crate::execute::scope::{DispatchTarget, prepare_effective_scope};
use crate::execute::support::advance_turn;
use crate::lua::{
    MessageRecord, OverflowReason, current_tool_bindings, is_context_overflow, precheck,
    project_messages, resolve_model_binding,
};
use crate::model::ModelBinding;
use crate::model::{Completion, CompletionResult, ToolCall};
use crate::{Error, Result};
use promptforge_api_types::emitter::Emitter;
use promptforge_api_types::event::lifecycle;

use super::builtins::{advertise_task_builtins, scope_halves, task_allowlist};
use super::{ChainIndex, Continuation, Scheduler};

/// The answer for a round refused as too large by `reason`'s gate, before
/// or by the provider: no round ran, so every other field is absent.
fn overflow_result(reason: OverflowReason) -> ChatResult {
    ChatResult {
        overflow: true,
        overflow_reason: Some(reason),
        reply: None,
        empty_detail: None,
        tool_calls: None,
        finish_reason: None,
        model: String::new(),
        metrics: None,
    }
}

/// Assembles one round's [`CallMetrics`] from everything the completion
/// measured, or `None` when nothing was measured.
fn call_metrics(completion: &Completion) -> Option<CallMetrics> {
    let metrics = CallMetrics {
        usage: completion.usage().cloned(),
        llama: completion.llama_timings().cloned(),
        vllm: completion.vllm_metrics().cloned(),
        client: completion.client_timing().cloned(),
    };
    let measured = metrics.usage.is_some()
        || metrics.llama.is_some()
        || metrics.vllm.is_some()
        || metrics.client.is_some();
    measured.then_some(metrics)
}

/// How one `chat` dispatch resolved: a round issued as an effect and
/// parked on the pending table, or an answer settled without leaving (the
/// precheck overflow).
enum ChatDispatch {
    Issued,
    Answered(Answer<Error>),
}

impl Scheduler {
    /// Dispatches a `chat` request: one tool-capable model round over the
    /// author's message list. An issued round parks the chain in the
    /// pending table; a precheck overflow answers the round on the spot
    /// with the overflow flag; every preparation failure - the binding,
    /// the scope, the projection, the client - is the call's answer,
    /// resumed into the caller so an author `pcall` catches it exactly as
    /// on the other dispatch paths.
    pub(super) fn dispatch_chat(
        &mut self,
        id: ChainIndex,
        messages: &[MessageRecord],
        binding: Option<ModelBinding>,
        model: Option<&str>,
        tools: Option<&[String]>,
    ) {
        match self.prepare_chat(id, messages, binding, model, tools) {
            Ok(ChatDispatch::Issued) => {}
            Ok(ChatDispatch::Answered(answer)) => {
                self.chains[id.index()].incoming = Some(answer);
                self.ready.push_back(id);
            }
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::Chat(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of chat dispatch: the binding (the loop shim's
    /// leading handle when it named one, else `model: None` is the
    /// section's current model and an alias is its frozen binding), the
    /// call-time tool scope recorded on the chain as `advertised`, the
    /// per-dispatch projection, the context precheck, and the issued
    /// effect.
    fn prepare_chat(
        &mut self,
        id: ChainIndex,
        messages: &[MessageRecord],
        binding: Option<ModelBinding>,
        model: Option<&str>,
        tools: Option<&[String]>,
    ) -> Result<ChatDispatch> {
        let chain = &self.chains[id.index()];
        let section = chain.section_name().to_owned();
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        let vm = frame.vm()?;
        let binding = match (binding, model) {
            (Some(binding), _) => binding,
            (None, None) => resolve_model_binding(chain.ctx.models(), &vm.model_runtime)?
                .ok_or_else(|| Error::ModelRequired {
                    section: section.clone(),
                })?,
            (None, Some(alias)) => chain.ctx.models().binding(alias)?.ok_or_else(|| {
                Error::Lua(format!("model alias {alias:?} has no frozen binding"))
            })?,
        };
        let tool_set = chain.ctx.tool_set_snapshot()?;
        // The scope is read at call time: `tools.add` and `tools.add_local`
        // calls since the last model operation shape this round's
        // advertised set.
        let effective = current_tool_bindings(&tool_set, &vm.tool_runtime)?;
        let local_schemas = vm.local_tool_schemas()?;
        let emitter = frame.reporting_handles().emitter;
        let (bound, locals) = scope_halves(tools, effective, local_schemas, &tool_set)?;
        let (mut schemas, mut dispatch) =
            prepare_effective_scope(&bound, &locals, emitter.as_ref(), &section)?;
        // `tools.allow_tasks` is the section's opt-in: while its allowlist
        // is set, every round offers the model its task built-ins.
        if let Some(allowlist) = task_allowlist(vm)? {
            advertise_task_builtins(&mut schemas, &mut dispatch, &allowlist)?;
        }
        // The projection failure reports a failed turn before its call-site
        // error resumes into Lua, the loop's precedent.
        let conversation = match project_messages(messages) {
            Ok(conversation) => conversation,
            Err(error) => {
                emitter.report(&section, lifecycle::MODEL_TURN_FAILED);
                return Err(Error::from(error));
            }
        };
        let context = binding.context();
        self.chains[id.index()].advertised = Some(dispatch);
        // The pre-dispatch precheck: an over-window request never leaves.
        // The refusal is the round's answer - the overflow flag - and is
        // observed as a failed turn, exactly as the loop reported it.
        if let Err(reason) = precheck(&conversation, context) {
            emitter.report(&section, lifecycle::MODEL_TURN_FAILED);
            return Ok(ChatDispatch::Answered(Answer::Chat(Ok(Box::new(
                overflow_result(reason),
            )))));
        }
        let effect = Effect::Chat {
            options: binding.completion_options(),
            binding,
            messages: conversation,
            tools: schemas,
            stream: true,
        };
        self.issue(id, effect, Continuation::Chat);
        Ok(ChatDispatch::Issued)
    }

    /// Classifies one arrived chat round into the chain's answer, emitting
    /// the round's events through the chain's task-scoped emitter.
    ///
    /// A provider context rejection is the overflow answer under a failed
    /// turn. An empty reply is a completed round with the reply absent -
    /// the turn advances and completes - so the shim applies its exit
    /// rules against `finish_reason`. Every other failure is a failed turn
    /// and the call's error. A served completion advances the turn, fires
    /// the debug capture pair and the completion observation, reports the
    /// thinking side channel, then the reply (with the truncation
    /// observation on a `length` finish) or the tool-call batch; a
    /// requested tool outside the scope this chain advertised for the round
    /// fails the call as out of scope after a failed-tool-call observation.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the parked chain has lost its frame
    /// or the scope it advertised for the round, or the run's tool set
    /// cannot be read.
    pub(super) fn accept_chat(
        &self,
        id: ChainIndex,
        result: Result<Box<Completion>>,
    ) -> Result<Answer<Error>> {
        let chain = &self.chains[id.index()];
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        let handles = frame.reporting_handles();
        let round = Round {
            section: chain.section_name().to_owned(),
            emitter: handles.emitter,
            turns: handles.turns,
        };
        let completion = match result {
            Ok(completion) => completion,
            Err(error) => return Ok(Answer::Chat(round.failed(error))),
        };
        // A round trip that produced a reply is a turn, whether the reply
        // is text or a batch of tool calls.
        let turn = advance_turn(&round.turns);
        let (outcome, served) = round.served(*completion, turn);
        let result = match outcome {
            CompletionResult::Text(text) => Ok(round.text_reply(&served, turn, text)),
            CompletionResult::ToolCalls(calls) => {
                // The scope is recorded before the round is spawned; its
                // absence is a scheduler fault, never an author-visible
                // out-of-scope refusal.
                let advertised = chain
                    .advertised
                    .as_ref()
                    .ok_or(Error::internal("a parked chat round recorded its scope"))?;
                let global_exists = |name: &str| -> Result<bool> {
                    Ok(chain.ctx.tool_set_snapshot()?.binding(name).is_some())
                };
                round.tool_calls(&served, turn, &calls, advertised, global_exists)?
            }
            // `CompletionResult` is `#[non_exhaustive]` across the crate
            // boundary: an outcome this build does not recognize can be
            // neither resumed nor promoted to an answer.
            _ => Err(Error::internal("unrecognized completion outcome")),
        };
        Ok(Answer::Chat(result.map(Box::new)))
    }
}

/// One arrived round's reporting context: the chain's section label, its
/// task-scoped emitter, and the turn counter it advances (a task chain's
/// own, so its turns count against its own cap).
struct Round {
    section: String,
    emitter: Arc<Emitter>,
    turns: Arc<AtomicU32>,
}

/// What a served completion reports once the turn has advanced and the
/// completion observation has fired: the pieces both the text and the
/// tool-call arms carry into the round's answer.
struct Served {
    finish_reason: Option<String>,
    model: String,
    metrics: Option<CallMetrics>,
}

impl Round {
    /// Classifies a round that produced no completion. A provider context
    /// rejection is the overflow answer under a failed turn. An empty reply
    /// is a completed round with the reply absent - the turn advances and
    /// completes - because whether it is the model's clean exit or a
    /// failure depends on the rounds before it, which only the shim knows;
    /// no debug capture fires because the failed completion carries no
    /// request/response bodies to record. Every other failure is a failed
    /// turn and the call's error.
    fn failed(&self, error: Error) -> std::result::Result<Box<ChatResult>, Error> {
        match error {
            Error::Backend { status, body } if is_context_overflow(status, &body) => {
                self.emitter
                    .report(&self.section, lifecycle::MODEL_TURN_FAILED);
                Ok(Box::new(overflow_result(OverflowReason::Provider)))
            }
            Error::EmptyModelReply {
                detail: phrase,
                finish_reason,
                ..
            } => {
                advance_turn(&self.turns);
                self.emitter
                    .report(&self.section, lifecycle::MODEL_TURN_COMPLETED);
                Ok(Box::new(ChatResult {
                    overflow: false,
                    overflow_reason: None,
                    reply: None,
                    empty_detail: Some(phrase.into_owned()),
                    tool_calls: None,
                    finish_reason,
                    model: String::new(),
                    metrics: None,
                }))
            }
            error => {
                self.emitter
                    .report(&self.section, lifecycle::MODEL_TURN_FAILED);
                Err(error)
            }
        }
    }

    /// Reports a served completion's round-level events - the debug
    /// capture pair, the completion event, and the thinking side channel -
    /// and dissolves the completion into its outcome and what the answer
    /// arms report beside it.
    fn served(&self, completion: Completion, turn: u32) -> (CompletionResult, Served) {
        // Extracted before the debug capture, which moves the request body
        // out of the completion.
        let metrics = call_metrics(&completion);
        let model = completion.model().to_owned();
        let thinking = completion
            .reasoning_content()
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
        let finish_reason = completion.finish_reason().map(str::to_owned);
        if self.emitter.captures_debug() {
            self.emitter
                .request(&self.section, turn, completion.request_body);
            self.emitter.response(
                &self.section,
                turn,
                completion.response_body.clone(),
                completion.finish_reason.clone(),
                completion.reasoning_content.clone(),
            );
        }
        self.emitter
            .report(&self.section, lifecycle::MODEL_TURN_COMPLETED);
        // The content reports every host transcript is built from: the
        // thinking side channel first, then the reply or the tool-call
        // batch, each with model and metrics.
        if let Some(thinking) = &thinking {
            self.emitter.thinking(&self.section, turn, &model, thinking);
        }
        (
            completion.result,
            Served {
                finish_reason,
                model,
                metrics,
            },
        )
    }

    /// Reports a text reply (with the truncation observation on a `length`
    /// finish) and builds its answer.
    fn text_reply(&self, served: &Served, turn: u32, text: String) -> ChatResult {
        if served.finish_reason.as_deref() == Some("length") {
            self.emitter
                .report(&self.section, lifecycle::MODEL_TURN_TRUNCATED);
        }
        self.emitter.assistant_reply(
            &self.section,
            turn,
            &text,
            served.finish_reason.as_deref(),
            &served.model,
            served.metrics.as_ref(),
        );
        ChatResult {
            overflow: false,
            overflow_reason: None,
            reply: Some(text),
            empty_detail: None,
            tool_calls: None,
            finish_reason: served.finish_reason.clone(),
            model: served.model.clone(),
            metrics: served.metrics.clone(),
        }
    }

    /// Reports a tool-call batch and builds its answer, after the scope
    /// gate: every requested name must be one the chain advertised for
    /// this round, else the call fails as out of scope under a failed
    /// tool-call observation. The batch resumes unexecuted; the shim
    /// dispatches each call.
    ///
    /// # Errors
    /// Returns the error `global_exists` reports when the run's tool set
    /// cannot be read.
    fn tool_calls(
        &self,
        served: &Served,
        turn: u32,
        calls: &[ToolCall],
        advertised: &BTreeMap<String, DispatchTarget>,
        global_exists: impl Fn(&str) -> Result<bool>,
    ) -> Result<std::result::Result<ChatResult, Error>> {
        let events: Vec<ToolCallEvent> = calls
            .iter()
            .map(|call| ToolCallEvent {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect();
        self.emitter
            .assistant_tool_calls(&self.section, turn, &served.model, &events);
        if let Some(rogue) = calls
            .iter()
            .find(|call| !advertised.contains_key(&call.name))
        {
            self.emitter
                .report(&self.section, lifecycle::TOOL_CALL_FAILED);
            return Ok(Err(Error::OutOfScopeToolCall {
                name: rogue.name.clone(),
                global_exists: global_exists(&rogue.name)?,
                in_scope: advertised.keys().cloned().collect(),
            }));
        }
        Ok(Ok(ChatResult {
            overflow: false,
            overflow_reason: None,
            reply: None,
            empty_detail: None,
            tool_calls: Some(events),
            finish_reason: served.finish_reason.clone(),
            model: served.model.clone(),
            metrics: served.metrics.clone(),
        }))
    }
}
