//! The `tool_call` arm: one dispatched tool call yielded by a section VM,
//! script-initiated (`tools.call`) or model-issued (the loop shim, with the
//! model's `call_id`).
//!
//! Three things resolve on the driver thread before any leaf work: the
//! five reserved model built-in names (`task`, `task_cancel`, `task_status`,
//! `task_events`, `await_tasks`) are recognized before alias lookup and
//! answer as unbound until their arms land; a local Lua tool is answered
//! inline on the parked chain's VM, since its handler is Lua on that VM
//! and no leaf work exists to spawn; a bound tool resolves against the
//! run's full bound catalog and its dispatch spawns onto the answer
//! channel. `call_id: Some` always resumes with content - a tool's own
//! failure becomes untrusted failure text - and `ToolResult` fires under
//! the id; `call_id: None` keeps the raise-at-call-site behavior, and its
//! `ToolResult` fires under no id.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::execute::protocol::{Answer, ToolCallOutcome};
use crate::lua::{
    ModelReport, ScriptReport, SectionVm, ToolCallCounts, current_tool_bindings,
    dispatch_model_tool, dispatch_tool,
};
use crate::observe::{Observer, detail};
use crate::{Error, Result, cancel};

use super::dispatch::unbound_tool_call;
use super::{Arrival, ChainIndex, RequestId, Scheduler};

/// The model built-in names the `tasks` namespace answers from this arm,
/// recognized before alias lookup so no bound or local tool can shadow
/// them. Until their arms land they answer as unbound.
const RESERVED_TOOL_NAMES: [&str; 5] = [
    "task",
    "task_cancel",
    "task_status",
    "task_events",
    "await_tasks",
];

/// How one `tool_call` dispatch resolved: a spawned bound dispatch parked
/// on the pending table, or an answer settled on the driver thread (a
/// local Lua tool, run on the parked chain's VM).
enum ToolCallDispatch {
    Spawned(RequestId, tokio::task::JoinHandle<()>),
    Answered(Answer<Error>),
}

/// Answers a call to a local Lua tool on the parked chain's VM: the counts
/// seed and increment (dispatch attempted, even if the handler then
/// fails), the handler run, the succeeded/failed observation, and the
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
    reason = "the inline answer names the same run coordinates the spawned dispatch bodies do"
)]
fn answer_local_tool(
    vm: &SectionVm,
    counts: &ToolCallCounts,
    alias: &str,
    args: &serde_json::Value,
    call_id: Option<&str>,
    report: ScriptReport,
    observer: &dyn Observer,
    execution: &str,
    section: &str,
) -> Result<ToolCallOutcome> {
    counts.ensure(alias)?;
    counts.increment(alias)?;
    // The handler is synchronous Lua on this thread, so there is no future
    // to race against cancellation; the VM's instruction hook polls the
    // cancel flag, so a stuck handler still aborts on cancellation.
    let result = vm.call_local_tool(alias, args).map_err(Error::from);
    observer.observe(
        execution,
        section,
        if result.is_ok() {
            detail::TOOL_CALL_SUCCEEDED
        } else {
            detail::TOOL_CALL_FAILED
        },
    );
    let text = result?;
    observer.on_tool_result(
        execution,
        section,
        report.chain_id,
        report.depth,
        report.turn,
        call_id.unwrap_or(""),
        alias,
        &text,
        true,
    );
    Ok(ToolCallOutcome::Plain(text))
}

impl Scheduler<'_> {
    /// Dispatches a `tool_call` request. A spawned bound dispatch parks the
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
            Ok(ToolCallDispatch::Spawned(request_id, task)) => {
                self.io_tasks.insert(request_id, task);
                self.pending.insert(request_id, id);
            }
            Ok(ToolCallDispatch::Answered(answer)) => {
                self.chains[id.index()].incoming = Some(answer);
                self.ready.push_back(id);
            }
            Err(error) => {
                self.chains[id.index()].incoming = Some(Answer::ToolCallResult(Err(error)));
                self.ready.push_back(id);
            }
        }
    }

    /// The fallible half of tool-call dispatch: the reserved-name check,
    /// the one-time counts install, the local-tool inline answer, then the
    /// alias resolved against the run's full bound tool catalog (the
    /// section's effective scope shapes what the model is offered, and the
    /// author's own script is not the model, so the scope does not gate it;
    /// the model-advertised set stays section-scoped) and the spawned
    /// dispatch: the model-issued body under a `call_id`, else the script
    /// body classified by the binding's declared output kind at completion.
    fn prepare_tool_call(
        &mut self,
        id: ChainIndex,
        alias: &str,
        args: serde_json::Value,
        call_id: Option<String>,
    ) -> Result<ToolCallDispatch> {
        let chain = &mut self.chains[id.index()];
        let tool_set = chain.ctx.tool_set_snapshot()?;
        // The reservation wins over every lookup: a bound or local tool
        // registered under one of these names is never reachable here.
        if RESERVED_TOOL_NAMES.contains(&alias) {
            return Err(unbound_tool_call(&tool_set, alias));
        }
        let ctx = chain.ctx.clone();
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution().to_owned();
        let section = chain.section_name().to_owned();
        let nonce = chain.ctx.nonce().clone();
        let report = ScriptReport {
            chain_id: id.0,
            // The call depth is capped at MAX_CALL_DEPTH, far inside
            // u32; the saturation is a defensive no-op.
            depth: u32::try_from(chain.call_depth).unwrap_or(u32::MAX),
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
                observer.as_ref(),
                &execution,
                &section,
            );
            return Ok(ToolCallDispatch::Answered(Answer::ToolCallResult(outcome)));
        }
        let Some(binding) = tool_set.binding(alias).cloned() else {
            return Err(unbound_tool_call(&tool_set, alias));
        };
        // The counts seed from the section's effective scope; a bound alias
        // outside it must still be seeded here, because the shared dispatch
        // body's increment errors on an unseeded alias.
        counts.ensure(binding.alias())?;
        let output_kind = binding.output_kind;
        let request_id = RequestId(self.next_request);
        self.next_request += 1;
        let tx = self.answer_tx.clone();
        // A spawned task does not inherit the cancel task-local; the
        // current handle rides into the task explicitly so the shared
        // dispatch body's cancel race stays armed there. The driver also
        // aborts the task handle on cancellation, so both paths end a slow
        // tool promptly.
        let cancel = cancel::current();
        let task = tokio::spawn(async move {
            let result = cancel::maybe_scope(cancel, async {
                match call_id {
                    // Model-issued: the content always resumes, plain -
                    // it is the tool record's text for the next round,
                    // never classified by output kind.
                    Some(call_id) => {
                        let report = ModelReport {
                            script: report,
                            call_id,
                        };
                        dispatch_model_tool(
                            &binding,
                            args,
                            Some(&counts),
                            &nonce,
                            observer.as_ref(),
                            &execution,
                            &section,
                            &report,
                        )
                        .await
                        .map(|outcome| ToolCallOutcome::Plain(outcome.into_content()))
                        .map_err(Error::from)
                    }
                    None => match dispatch_tool(
                        &binding,
                        args,
                        Some(&counts),
                        &nonce,
                        observer.as_ref(),
                        &execution,
                        &section,
                        Some(report),
                    )
                    .await
                    {
                        Ok(outcome) => ToolCallOutcome::from_dispatch(
                            output_kind,
                            binding.alias(),
                            outcome.into_content(),
                        )
                        .map_err(Error::from),
                        Err(error) => Err(Error::from(error)),
                    },
                }
            })
            .await;
            // A send fails only when the driver is gone (a cancelled run);
            // the answer is then moot.
            let _ = tx.send((request_id, Arrival::Answer(Answer::ToolCallResult(result))));
        });
        Ok(ToolCallDispatch::Spawned(request_id, task))
    }
}
