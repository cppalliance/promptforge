//! The `tool_call` arm: one dispatched tool call yielded by a section VM,
//! script-initiated (`tools.call`) or model-issued (the loop shim, with the
//! model's `call_id`), plus the `local_tool_done` arm that closes a local
//! tool call.
//!
//! Three things resolve on the driver thread before any leaf work: the
//! five reserved model built-in names (`task`, `task_cancel`, `task_status`,
//! `task_events`, `await_tasks`) are recognized before alias lookup - a
//! model-issued call to the first three is answered by the `builtins`
//! module over the task arena, `await_tasks` by its own module (answered
//! at once or parked on the chain's model tasks), `task_events` by its
//! own module (issued as a `TaskEvents` effect the host answers from its
//! log), and a script call to any of them answers as unbound; a local Lua
//! tool is answered with its handler; a bound tool resolves against the
//! run's full bound catalog, its attempt is counted, and the call is
//! issued as a `ToolCall` effect whose answer the driver applies through
//! the shared dispatch body. `call_id: Some` always resumes a bound tool
//! with content - a tool's own failure becomes untrusted failure text -
//! and `ToolResult` fires under the id; `call_id: None` keeps the
//! raise-at-call-site behavior, and its `ToolResult` fires under no id.
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

use crate::execute::protocol::{Answer, LocalToolOutcome, ToolCallOutcome};
use crate::execute::run::Effect;
use crate::execute::section_context::LocalCall;
use crate::lua::{ScriptReport, current_tool_bindings};
use crate::{Error, Result};
use promptforge_types::event::lifecycle;
use promptforge_types::tools::OutputTrust;

use super::builtins::is_task_builtin;
use super::dispatch::unbound_tool_call;
use super::{ChainIndex, Continuation, Scheduler, ToolCallContinuation};

/// The model built-in names the `tasks` namespace answers from this arm,
/// recognized before alias lookup so no bound or local tool can shadow
/// them. A model-issued call to any of them is answered over the task
/// arena (`task_events` through a host-answered effect); a script call
/// to any of them is unbound.
const RESERVED_TOOL_NAMES: [&str; 5] = [
    "task",
    "task_cancel",
    "task_status",
    "task_events",
    "await_tasks",
];

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
    ) {
        match self.prepare_tool_call(id, alias, args, call_id) {
            // An issued call is parked on the pending table; a parked
            // wait was recorded on the chain, and a member's end or the
            // timer's firing answers it.
            Ok(ToolCallDispatch::Issued | ToolCallDispatch::Parked) => {}
            Ok(ToolCallDispatch::Answered(answer)) => {
                self.answer_inline(id, answer);
            }
            // The caller runs first and the task when it suspends, the
            // order `tasks.spawn` keeps.
            Ok(ToolCallDispatch::Started(answer, child)) => {
                self.answer_inline(id, answer);
                self.ready.push_back(child);
            }
            Err(error) => {
                self.answer_inline(id, Answer::ToolCallResult(Err(error)));
            }
        }
    }

    /// The fallible half of tool-call dispatch: the reserved-name check
    /// (a model-issued task built-in answered over the arena, a script
    /// call to a reserved name unbound), the one-time counts install, the
    /// local tool's handler answer, then the alias resolved against the
    /// run's full bound tool catalog (the section's effective scope shapes
    /// what the model is offered, and the author's own script is not the
    /// model, so the scope does not gate it; the model-advertised set stays
    /// section-scoped), the attempt counted, and the issued effect. The
    /// answer's rules - the model-issued body under a `call_id`, else the
    /// script body classified by the binding's declared output kind - are
    /// the continuation's, applied when the answer lands.
    fn prepare_tool_call(
        &mut self,
        id: ChainIndex,
        alias: &str,
        args: serde_json::Value,
        call_id: Option<String>,
    ) -> Result<ToolCallDispatch> {
        let tool_set = self.chains[id.index()].ctx.tool_set_snapshot()?;
        // The reservation wins over every lookup: a bound or local tool
        // registered under one of these names is never reachable here. The
        // built-ins serve the model; the author's own script reaches the
        // arena through the `tasks` namespace, so a script call stays
        // unbound.
        if RESERVED_TOOL_NAMES.contains(&alias) {
            if let Some(call_id) = call_id.as_deref().filter(|_| is_task_builtin(alias)) {
                return self.answer_task_builtin(id, alias, &args, call_id);
            }
            return Err(unbound_tool_call(&tool_set, alias));
        }
        let chain = &mut self.chains[id.index()];
        let ctx = chain.ctx.clone();
        let report = ScriptReport {
            turn: chain.ctx.turns().load(Ordering::Relaxed),
        };
        let frame = chain
            .frame
            .as_mut()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        let effective = current_tool_bindings(&tool_set, &frame.vm()?.tool_runtime)?;
        let counts = frame.script_call_counts(&ctx, &effective)?;
        if frame.vm()?.has_local_tool(alias)? {
            // The attempt counts at dispatch, before the handler runs, so
            // a handler that then fails still counts.
            counts.ensure(alias)?;
            counts.increment(alias)?;
            frame.push_local_call(LocalCall {
                alias: alias.to_owned(),
                call_id,
                report,
            });
            return Ok(ToolCallDispatch::Answered(Answer::ToolCallResult(Ok(
                ToolCallOutcome::Local {
                    alias: alias.to_owned(),
                    args,
                },
            ))));
        }
        let Some(binding) = tool_set.binding(alias).cloned() else {
            return Err(unbound_tool_call(&tool_set, alias));
        };
        // The counts seed from the section's effective scope; a bound alias
        // outside it must still be seeded here, because the increment
        // errors on an unseeded alias. The attempt counts at dispatch -
        // before the tool runs, so a cancelled dispatch still counts,
        // exactly as the shared body has always counted it.
        counts.ensure(binding.alias())?;
        counts.increment(binding.alias())?;
        let effect = Effect::ToolCall {
            tool: binding.id().clone(),
            alias: binding.alias().to_owned(),
            args,
        };
        let resume = Continuation::ToolCall(ToolCallContinuation {
            binding,
            report,
            call_id,
        });
        self.issue(id, effect, resume);
        Ok(ToolCallDispatch::Issued)
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
