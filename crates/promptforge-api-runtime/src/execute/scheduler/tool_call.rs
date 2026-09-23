//! The `tool_call` arm: one dispatched tool call yielded by a section VM,
//! script-initiated (`tools.call`) or model-issued (the loop shim, with the
//! model's `call_id`).
//!
//! Three things resolve on the driver thread before any leaf work: the
//! five reserved model built-in names (`task`, `task_cancel`, `task_status`,
//! `task_events`, `await_tasks`) are recognized before alias lookup - a
//! model-issued call to the first three is answered by the `builtins`
//! module over the task arena, `await_tasks` by its own module (answered
//! at once or parked on the chain's model tasks), `task_events` by its
//! own module (issued as a `TaskEvents` effect the host answers from its
//! log), and a script call to any of them answers as unbound; a
//! local Lua tool is
//! answered inline on the parked chain's VM, since its handler is Lua on
//! that VM and no leaf work exists to issue; a bound tool resolves against
//! the run's full bound catalog, its attempt is counted, and the call is
//! issued as a `ToolCall` effect whose answer the driver applies through
//! the shared dispatch body. `call_id: Some` always resumes with content -
//! a tool's own failure becomes untrusted failure text - and `ToolResult`
//! fires under the id; `call_id: None` keeps the raise-at-call-site
//! behavior, and its `ToolResult` fires under no id.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::execute::protocol::{Answer, ToolCallOutcome};
use crate::execute::run::Effect;
use crate::lua::{ScriptReport, SectionVm, ToolCallCounts, current_tool_bindings};
use crate::{Error, Result};
use promptforge_api_types::emitter::Emitter;
use promptforge_api_types::event::lifecycle;
use promptforge_api_types::tools::OutputTrust;

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
/// (a local Lua tool, run on the parked chain's VM; a task built-in), such
/// an answer plus the task chain a `task` built-in started, enqueued
/// behind the caller, or the chain parked in the model's `await_tasks` on
/// its live tasks, answered when one ends or its timer fires.
pub(super) enum ToolCallDispatch {
    Issued,
    Answered(Answer<Error>),
    Started(Answer<Error>, ChainIndex),
    Parked,
}

/// Answers a call to a local Lua tool on the parked chain's VM: the counts
/// seed and increment (dispatch attempted, even if the handler then
/// fails), the handler run, the succeeded/failed event, and the
/// `ToolResult` report - trusted, since the prompt author wrote the
/// handler and its output passes verbatim - under the model-issued call
/// id, or no id for a script call. No leaf work is issued. A handler
/// failure is the call's error for both forms: it is the author's own
/// program failing, not a tool's own failure, exactly as the Rust loop
/// treats it.
///
/// # Errors
/// Returns the counts' own error, or the handler's failure.
#[expect(
    clippy::too_many_arguments,
    reason = "the inline answer names the same call coordinates the spawned dispatch bodies do"
)]
fn answer_local_tool(
    vm: &SectionVm,
    counts: &ToolCallCounts,
    alias: &str,
    args: &serde_json::Value,
    call_id: Option<&str>,
    report: ScriptReport,
    emitter: &Emitter,
    section: &str,
) -> Result<ToolCallOutcome> {
    counts.ensure(alias)?;
    counts.increment(alias)?;
    // The handler is synchronous Lua on this thread, so there is no future
    // to race against cancellation; the VM's instruction hook polls the
    // cancel flag, so a stuck handler still aborts on cancellation.
    let result = vm.call_local_tool(alias, args).map_err(Error::from);
    emitter.report(
        section,
        if result.is_ok() {
            lifecycle::TOOL_CALL_SUCCEEDED
        } else {
            lifecycle::TOOL_CALL_FAILED
        },
    );
    let text = result?;
    emitter.tool_result(
        section,
        report.turn,
        call_id.unwrap_or(""),
        alias,
        &text,
        OutputTrust::Trusted,
    );
    Ok(ToolCallOutcome::Plain(text))
}

impl Scheduler {
    /// Dispatches a `tool_call` request. An issued bound call parks the
    /// chain in the pending table; a local Lua tool's answer resumes the
    /// chain on the spot; every preparation failure - a reserved name, an
    /// unbound alias, the counts install, a local handler's failure - is
    /// the call's answer, resumed into the caller so an author `pcall`
    /// catches it exactly as a tool failure.
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
    /// local-tool inline answer, then the alias resolved against the run's
    /// full bound tool catalog (the section's effective scope shapes what
    /// the model is offered, and the author's own script is not the model,
    /// so the scope does not gate it; the model-advertised set stays
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
        let emitter = Arc::clone(chain.ctx.emitter());
        let section = chain.section_name().to_owned();
        let report = ScriptReport {
            turn: chain.ctx.turns().load(Ordering::Relaxed),
        };
        let frame = chain
            .frame
            .as_mut()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        let effective = current_tool_bindings(&tool_set, &frame.vm()?.tool_runtime)?;
        let counts = frame.script_call_counts(&ctx, &effective)?;
        let vm = frame.vm()?;
        // A local tool is a Lua function on this section VM: it is answered
        // here, on the parked chain's VM, with no leaf work.
        if vm.has_local_tool(alias)? {
            let outcome = answer_local_tool(
                vm,
                &counts,
                alias,
                &args,
                call_id.as_deref(),
                report,
                emitter.as_ref(),
                &section,
            );
            return Ok(ToolCallDispatch::Answered(Answer::ToolCallResult(outcome)));
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
}
