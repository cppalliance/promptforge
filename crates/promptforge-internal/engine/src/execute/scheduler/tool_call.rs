//! The `tool_call` arm: one dispatched tool call yielded by a section VM,
//! script-initiated (`tools.call`) or model-issued (the loop shim, with the
//! model's `call_id`), plus the `local_tool_done` arm that closes a local
//! tool call.
//!
//! Each call's route is decided once, by `tool_route`, on the driver
//! thread before any leaf work. The four reserved model built-in names
//! (`task`, `task_cancel`, `task_status`, `await_tasks`) are recognized
//! before any lookup - a model-issued call to the first three is answered
//! by the `builtins` module over the task arena, `await_tasks` by its own
//! module (answered at once or parked on the chain's model tasks), and a
//! script call to any of them answers as unbound; a local Lua tool is
//! answered with its handler; anything else is a catalog tool, which
//! resolves against the run's offering (a model call by wire name, a
//! script call by canonical id, which also reaches a catalog tool left
//! out of the offering), has its attempt counted, and is issued as a
//! `ToolCall` effect, carrying an identity forked from the chain's
//! access, whose answer the driver applies through the shared dispatch
//! body. The call's `CallSite` names who asked. A model-issued call,
//! alone or batched, always resumes a bound tool with content - a tool's
//! own failure becomes untrusted failure text - and `ToolResult` fires
//! under the id and the turn of the round that requested the call; a
//! script call keeps the raise-at-call-site behavior, and its
//! `ToolResult` fires under no id and the turn at dispatch.
//!
//! The loop sends a round's unanswered calls together as one
//! `tool_calls` request. The arm walks them in the model's order and
//! issues each catalog call at once into a slot of the chain's batch,
//! stopping at the first call whose route is a reserved built-in name or
//! a local tool. A call that fails to route or to prepare fills its slot
//! with the error and stops the batch too. Each landed answer fills its call's
//! slot, and the chain is answered once no slot is open: every text in
//! call order, or the first failure in call order. Fewer texts than
//! calls tells the loop where the batch stopped, and the loop sends that
//! call alone as an ordinary `tool_call`. The calls in flight run
//! concurrently, each under its own forked identity.
//!
//! A local tool call is a two-yield handshake. The `tool_call` answer is
//! [`ToolCallOutcome::Local`], which hands the shim the handler, and the
//! frame opens a [`LocalCall`] for it. The shim runs the handler inside the
//! chain's block coroutine, so the handler's own suspending calls are
//! ordinary requests of this chain, then yields `local_tool_done`. That
//! arm closes the innermost open call and reports it: the succeeded or
//! failed observation and, on success, the trusted `ToolResult` under the
//! call id recorded at dispatch. A local call never issues leaf work of
//! its own.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::execute::protocol::{Answer, LocalToolOutcome, ToolCallOutcome, ToolCallRecord};
use crate::execute::run::{Effect, ToolCallOrigin, ToolCaller};
use crate::execute::section_context::LocalCall;
use crate::lua::{ScriptReport, ToolCallCounts, ToolSet, current_tool_bindings};
use crate::{Error, Result};
use promptforge_types::event::lifecycle;
use promptforge_types::tools::{OutputTrust, ToolId};
use promptforge_vfs::detail::{access_id, access_spawn};

use super::builtins::is_task_builtin;
use super::dispatch::unbound_tool_call;
use super::{CallSite, ChainIndex, Continuation, Scheduler, ToolCallContinuation, prompt_origin};

/// The model built-in names the `tasks` namespace answers from this arm,
/// recognized before any lookup so no catalog or local tool can shadow
/// them. A model-issued call to any of them is answered over the task
/// arena; a script call to any of them is unbound. The offering leaves
/// out a tool whose name is one of these.
pub(in crate::execute) const RESERVED_TOOL_NAMES: [&str; 4] =
    ["task", "task_cancel", "task_status", "await_tasks"];

/// How one `tool_call` dispatch resolved: a bound call issued as an effect
/// and parked on the pending table, an answer settled on the driver thread
/// (a local Lua tool's handler, a task built-in), such an answer plus the
/// task chain a `task` built-in started, enqueued behind the caller, or
/// the chain parked in the model's `await_tasks` on its live tasks,
/// answered when one ends or its timer fires.
pub(super) enum ToolCallDispatch {
    Issued,
    Answered(Answer<Error>),
    Started(Answer<Error>, ChainIndex),
    Parked,
}

/// Where one tool call is answered, decided before any catalog lookup so
/// a lone call and a batched one route alike.
enum ToolRoute {
    /// A reserved model built-in name: a model-issued task built-in is
    /// answered over the task arena, and anything else is unbound.
    Builtin,
    /// A local Lua tool of the chain's section VM, answered with its
    /// handler.
    Local,
    /// A catalog tool, resolved against the run's offering and issued as
    /// an effect.
    Catalog,
}

impl Scheduler {
    /// Dispatches a `tool_call` request. An issued bound call parks the
    /// chain in the pending table; a local Lua tool's handler resumes the
    /// chain on the spot; every preparation failure - a reserved name, an
    /// unbound alias, the counts install - is the call's answer, resumed
    /// into the caller so an author `pcall` catches it exactly as a tool
    /// failure.
    pub(super) fn dispatch_tool_call(
        &mut self,
        id: ChainIndex,
        alias: &str,
        args: serde_json::Value,
        call_id: Option<String>,
        turn: Option<u32>,
    ) {
        let site = match call_id {
            Some(call_id) => CallSite::Model { call_id, turn },
            None => CallSite::Script { turn },
        };
        match self.prepare_tool_call(id, alias, args, site) {
            // An issued call is parked on the pending table; a parked
            // wait was recorded on the chain, and a member's end or the
            // timer's firing answers it.
            Ok(ToolCallDispatch::Issued | ToolCallDispatch::Parked) => {}
            Ok(ToolCallDispatch::Answered(answer)) => {
                self.answer_inline(id, answer);
            }
            // The caller runs first and the task waits for admission, the
            // order `tasks.spawn` keeps.
            Ok(ToolCallDispatch::Started(answer, child)) => {
                self.answer_inline(id, answer);
                self.spawned.push_back(child);
            }
            Err(error) => {
                self.answer_inline(id, Answer::ToolCallResult(Err(error)));
            }
        }
    }

    /// Dispatches a `tool_calls` request: the unanswered calls of one
    /// model round, in the model's order. Each call before the first
    /// whose route is not the catalog is issued as a `CallSite::Batch`
    /// under the round's `turn` and its own slot, so every catalog call
    /// issues in this step. A call that fails to route or to prepare
    /// fills its slot with the error and stops the batch. The slots park
    /// on the chain, which is answered at once when none is open.
    pub(super) fn dispatch_tool_calls(
        &mut self,
        id: ChainIndex,
        calls: Vec<ToolCallRecord>,
        turn: u32,
    ) {
        let mut batch = Vec::with_capacity(calls.len());
        for (slot, call) in calls.into_iter().enumerate() {
            let issued = match self.tool_route(id, &call.name) {
                Ok(ToolRoute::Builtin | ToolRoute::Local) => break,
                Ok(ToolRoute::Catalog) => self.prepare_catalog_call(
                    id,
                    &call.name,
                    call.arguments,
                    CallSite::Batch {
                        call_id: call.id,
                        turn,
                        slot,
                    },
                ),
                Err(error) => Err(error),
            };
            match issued {
                Ok(()) => batch.push(None),
                Err(error) => {
                    batch.push(Some(Err(error)));
                    break;
                }
            }
        }
        self.chains[id.index()].batch = batch;
        self.answer_full_batch(id);
    }

    /// Decides the route of the call named `alias`, the one decision a
    /// lone call and a batch share. The reservation wins over every
    /// lookup: a bound or local tool registered under a reserved name is
    /// never reachable. A local tool of the chain's section VM wins over
    /// the catalog.
    fn tool_route(&self, id: ChainIndex, alias: &str) -> Result<ToolRoute> {
        if RESERVED_TOOL_NAMES.contains(&alias) {
            return Ok(ToolRoute::Builtin);
        }
        let frame = self.chains[id.index()]
            .frame
            .as_ref()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        if frame.vm()?.has_local_tool(alias)? {
            return Ok(ToolRoute::Local);
        }
        Ok(ToolRoute::Catalog)
    }

    /// The turn every report of a call at `site` carries: the requesting
    /// round's turn when the site names one, else the chain's turn
    /// counter at dispatch.
    fn call_turn(&self, id: ChainIndex, site: &CallSite) -> u32 {
        site.turn()
            .unwrap_or_else(|| self.chains[id.index()].ctx.turns().load(Ordering::Relaxed))
    }

    /// The chain's tool set and its section's tool-call counts, which the
    /// section's first call installs.
    fn section_call_counts(&mut self, id: ChainIndex) -> Result<(ToolSet, ToolCallCounts)> {
        let chain = &mut self.chains[id.index()];
        let tool_set = chain.ctx.tool_set_snapshot()?;
        let frame = chain
            .frame
            .as_mut()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        let effective = current_tool_bindings(&tool_set, &frame.vm()?.tool_runtime)?;
        let counts = frame.script_call_counts(&chain.ctx, &effective)?;
        Ok((tool_set, counts))
    }

    /// Fills `slot` of the batch the chain is parked on with a landed
    /// call's answer: a plain text as the text, a failure as the failure,
    /// and any other outcome, which a model-issued call never resumes
    /// with, as an internal error. Once no slot is open the chain is
    /// answered.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain holds no such slot,
    /// which only an answer the scheduler should have orphaned produces.
    pub(super) fn fill_batch_slot(
        &mut self,
        id: ChainIndex,
        slot: usize,
        answer: Answer<Error>,
    ) -> Result<()> {
        let landed = match answer {
            Answer::ToolCallResult(Ok(ToolCallOutcome::Plain(text))) => Ok(text),
            Answer::ToolCallResult(Err(error)) => Err(error),
            _ => Err(Error::internal(
                "a batched call lands as plain text or a failure",
            )),
        };
        let open = self.chains[id.index()]
            .batch
            .get_mut(slot)
            .filter(|open| open.is_none())
            .ok_or(Error::internal(
                "a batched call's answer fills an open slot of its chain's batch",
            ))?;
        *open = Some(landed);
        self.answer_full_batch(id);
        Ok(())
    }

    /// Answers the chain parked on a batch once no slot is open: every
    /// text in call order, or the first failure in call order.
    fn answer_full_batch(&mut self, id: ChainIndex) {
        let chain = &mut self.chains[id.index()];
        if chain.batch.iter().any(Option::is_none) {
            return;
        }
        let answer = std::mem::take(&mut chain.batch)
            .into_iter()
            .flatten()
            .collect::<Result<Vec<String>>>();
        self.answer_inline(id, Answer::ToolCallsResult(answer));
    }

    /// The fallible half of tool-call dispatch, by the call's route: a
    /// reserved name (a model-issued task built-in answered over the
    /// arena, a script call to a reserved name unbound), a local tool (the
    /// one-time counts install, the attempt counted, then the handler's
    /// answer), or a catalog tool, which `prepare_catalog_call` issues.
    ///
    /// Every report the call makes carries the turn of the model round
    /// that requested it, so a local handler earlier in the same batch
    /// that runs model rounds cannot shift it. A script call, or a
    /// model-issued one yielded without a turn, reports the counter's
    /// value at dispatch.
    fn prepare_tool_call(
        &mut self,
        id: ChainIndex,
        alias: &str,
        args: serde_json::Value,
        site: CallSite,
    ) -> Result<ToolCallDispatch> {
        match self.tool_route(id, alias)? {
            // The built-ins serve the model; the author's own script
            // reaches the arena through the `tasks` namespace, so a script
            // call stays unbound.
            ToolRoute::Builtin => {
                let tool_set = self.chains[id.index()].ctx.tool_set_snapshot()?;
                if let Some(call_id) = site.call_id().filter(|_| is_task_builtin(alias)) {
                    let turn = self.call_turn(id, &site);
                    return self.answer_task_builtin(id, alias, &args, call_id, turn);
                }
                Err(unbound_tool_call(&tool_set, alias))
            }
            ToolRoute::Local => {
                let report = ScriptReport {
                    turn: self.call_turn(id, &site),
                };
                let (_, counts) = self.section_call_counts(id)?;
                // The attempt counts at dispatch, before the handler runs, so
                // a handler that then fails still counts.
                counts.ensure(alias)?;
                counts.increment(alias)?;
                self.chains[id.index()]
                    .frame
                    .as_mut()
                    .ok_or(Error::internal("a live chain holds its frame"))?
                    .push_local_call(LocalCall {
                        alias: alias.to_owned(),
                        call_id: site.call_id().map(str::to_owned),
                        report,
                    });
                Ok(ToolCallDispatch::Answered(Answer::ToolCallResult(Ok(
                    ToolCallOutcome::Local {
                        alias: alias.to_owned(),
                        args,
                    },
                ))))
            }
            ToolRoute::Catalog => {
                self.prepare_catalog_call(id, alias, args, site)?;
                Ok(ToolCallDispatch::Issued)
            }
        }
    }

    /// Issues a catalog call: the one-time counts install, then the tool
    /// resolved - a model call's wire name against the run's offering, a
    /// script call's canonical id against the offering and then the full
    /// catalog, the id then standing as the alias of a tool the run could
    /// not offer (the section's effective scope shapes what the model is
    /// offered, and the author's own script is not the model, so the scope
    /// does not gate it); then the attempt counted under the tool's id,
    /// the call's identity forked from the chain's access, and the issued
    /// effect, whose origin names the model as caller for a `Model` or
    /// `Batch` site and the script for a `Script` site. The answer's
    /// rules - the model-issued body under the call id, else the script
    /// body classified by the binding's declared output kind - and a
    /// `Batch` site's slot are the continuation's, applied when the answer
    /// lands.
    fn prepare_catalog_call(
        &mut self,
        id: ChainIndex,
        alias: &str,
        args: serde_json::Value,
        site: CallSite,
    ) -> Result<()> {
        let report = ScriptReport {
            turn: self.call_turn(id, &site),
        };
        let (tool_set, counts) = self.section_call_counts(id)?;
        let ctx = &self.chains[id.index()].ctx;
        // A model reaches only the wire names a round advertised; a script
        // names a catalog tool by its canonical id, and only a script falls
        // back to a catalog tool the run could not offer.
        let binding = if site.call_id().is_some() {
            tool_set.wire_binding(alias)
        } else if ToolId::parse(alias).is_ok() {
            tool_set
                .offered_binding(alias)
                .or_else(|| ctx.catalog_binding(alias))
        } else {
            None
        }
        .cloned();
        let Some(binding) = binding else {
            return Err(unbound_tool_call(&tool_set, alias));
        };
        // The counts seed from the section's effective scope; a tool
        // outside it must still be seeded here, because the increment
        // errors on an unseeded id. The attempt counts at dispatch -
        // before the tool runs, so a cancelled dispatch still counts.
        let counted = binding.id().to_string();
        counts.ensure(&counted)?;
        counts.increment(&counted)?;
        // The call runs under an identity of its own, forked from the
        // chain's, so two calls in flight at once never share one sequence
        // of claims. A refused fork fails the call at its caller, and the
        // counts already taken stay taken.
        let prompt = self.prompt();
        let chain = &self.chains[id.index()];
        let access = access_spawn(
            chain.access()?,
            prompt_origin(&prompt, &binding.id().to_string(), chain.blocks(&prompt)),
        )
        .map_err(Error::store)?;
        let exec = access_id(&access);
        let origin = ToolCallOrigin {
            execution: chain.ctx.execution().to_owned(),
            section: chain.section_name().to_owned(),
            caller: match &site {
                CallSite::Script { .. } => ToolCaller::Script,
                CallSite::Model { .. } | CallSite::Batch { .. } => ToolCaller::Model,
            },
        };
        let effect = Effect::ToolCall {
            tool: binding.id().clone(),
            alias: binding.alias().to_owned(),
            args,
            origin,
            access: Arc::new(access),
        };
        let resume = Continuation::ToolCall(ToolCallContinuation {
            binding,
            report,
            site,
            exec,
        });
        self.issue(id, effect, resume);
        Ok(())
    }

    /// Dispatches a `local_tool_done` request: closes the chain's
    /// innermost open local tool call and answers it. A returned text
    /// reports `TOOL_CALL_SUCCEEDED` and a trusted `ToolResult` - the
    /// prompt author wrote the handler, so its output passes verbatim -
    /// under the call id (or none for a script call) and the turn recorded
    /// at dispatch, then resumes as the text. A rejected return reports
    /// `TOOL_CALL_FAILED` and resumes as its error. A raise reports
    /// `TOOL_CALL_FAILED` and resumes empty: the shim raises the handler's
    /// own value in its place.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain has no frame or no open
    /// local call, which only a shim bug produces.
    pub(super) fn dispatch_local_tool_done(
        &mut self,
        id: ChainIndex,
        outcome: LocalToolOutcome,
    ) -> Result<()> {
        let chain = &mut self.chains[id.index()];
        let emitter = Arc::clone(chain.ctx.emitter());
        let section = chain.section_name().to_owned();
        let call = chain
            .frame
            .as_mut()
            .ok_or(Error::internal("a live chain holds its frame"))?
            .pop_local_call()
            .ok_or(Error::internal(
                "a local tool's completion closes the call that opened it",
            ))?;
        let answer = match outcome {
            LocalToolOutcome::Returned(text) => {
                emitter.report(&section, lifecycle::TOOL_CALL_SUCCEEDED);
                emitter.tool_result(
                    &section,
                    call.report.turn,
                    call.call_id.as_deref().unwrap_or(""),
                    &call.alias,
                    None,
                    &text,
                    OutputTrust::Trusted,
                );
                Ok(ToolCallOutcome::Plain(text))
            }
            LocalToolOutcome::BadReturn(error) => {
                emitter.report(&section, lifecycle::TOOL_CALL_FAILED);
                Err(Error::from(error))
            }
            LocalToolOutcome::Raised => {
                emitter.report(&section, lifecycle::TOOL_CALL_FAILED);
                Ok(ToolCallOutcome::Plain(String::new()))
            }
        };
        self.answer_inline(id, Answer::ToolCallResult(answer));
        Ok(())
    }
}
