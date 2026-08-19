//! Execution of a single fanout arm: its owned payload, the terminal-observation
//! guard, and the thin adapter over the shared engine.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use serde_json::Value;

use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::execute::{
    BlockWalkContext, RunLimits, SectionFlow, SectionVmSetup, ToolAnalysis, VmSeed,
    attach_engine_infer_hook, now_rfc3339_checked, setup_section_vm, sys_json, walk_section_blocks,
};
use crate::lua::{LuaFanoutResult, LuaProgram, SectionVm, ToolBindings};
use crate::model::ModelBindings;
use crate::observe::{Observation, Observer, detail};
use crate::parser::Section;
use crate::store::StoreRef;
use crate::tools::SharedTools;
use crate::{Error, Result, cancel, subst};

use super::proxies::ProxyObserver;

/// Everything one spawned fanout arm owns for its independent execution.
pub(crate) struct ArmPayload {
    pub(crate) worker: Section,
    pub(crate) item: Value,
    pub(crate) index: usize,
    pub(crate) store: StoreRef,
    pub(crate) client: Option<GatewayClient>,
    pub(crate) args: String,
    pub(crate) execution: String,
    pub(crate) when: String,
    pub(crate) last_reply: Option<String>,
    pub(crate) shared: LuaProgram,
    pub(crate) bindings: ToolBindings,
    pub(crate) models: ModelBindings,
    pub(crate) analysis: ToolAnalysis,
    pub(crate) shared_tools: SharedTools,
    pub(crate) max_tool_iterations: usize,
    /// The run's resource limits: the arm's Lua ceilings come from it, and a
    /// lazily created gateway client inherits its HTTP timeout and body cap.
    pub(crate) limits: RunLimits,
    pub(crate) parent_id: usize,
    pub(crate) section_count: usize,
    pub(crate) turns: Arc<AtomicU32>,
    pub(crate) observer: Arc<ProxyObserver>,
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    /// Explicit cancellation handle carried across the spawn boundary, since a
    /// spawned arm does not inherit the parent task-local (PF-CANCEL-002).
    pub(crate) cancel: Option<cancel::CancelHandle>,
}

/// Emits exactly one distinct terminal observation per fanout arm.
///
/// The arm's normal exits call [`finish`](Self::finish) with the specific
/// terminal event (succeeded / exhausted / failed). If the arm's future is
/// instead dropped before finalizing - a sibling's hard error aborts it, or the
/// run is cancelled - `Drop` emits [`detail::FANOUT_ARM_CANCELLED`]. Exactly one
/// terminal event therefore fires for every arm (FANOUT-004).
pub(crate) struct ArmFinalizer {
    observer: Arc<dyn Observer>,
    execution: String,
    section: String,
    finished: bool,
}

impl ArmFinalizer {
    pub(crate) fn new(observer: Arc<dyn Observer>, execution: String, section: String) -> Self {
        Self {
            observer,
            execution,
            section,
            finished: false,
        }
    }

    pub(crate) fn finish(&mut self, event: Observation) {
        self.finished = true;
        self.emit(event);
    }

    fn emit(&self, event: Observation) {
        self.observer.observe(&self.execution, &self.section, event);
    }
}

impl Drop for ArmFinalizer {
    fn drop(&mut self) {
        if !self.finished {
            self.emit(detail::FANOUT_ARM_CANCELLED);
        }
    }
}

/// Runs one fanout arm to completion.
///
/// The arm is a thin adapter over the shared engine: the body builds the
/// engine's inputs from the payload (the `sys` JSON carrying `taskid` and the
/// parent section's `id`, the `item` seed, the proxy observer, the shared
/// turns counter), runs the setup half ([`setup_section_vm`]), installs the
/// `model:infer` hook with a lazy client source, and drives the shared block
/// walk ([`walk_section_blocks`]). The control globals stay stubbed - nested
/// execute/fanout/list_from_section fail loudly - and a [`SectionFlow::Jumped`]
/// maps to the jump-rejection error. [`SectionFlow::Returned`] and
/// [`SectionFlow::FellThrough`] map to [`LuaFanoutResult::success`];
/// [`Error::ToolLoopExhausted`] soft-degrades to the incomplete stub so one
/// stuck arm cannot kill sibling evidence.
///
/// VM teardown and the terminal arm observation happen in ONE epilogue
/// (FANOUT-006): the fallible body runs against a borrowed VM without any inline
/// teardown, then the epilogue tears the VM down once and records the single
/// distinct terminal event via [`ArmFinalizer`].
#[expect(
    clippy::too_many_lines,
    reason = "the arm adapter is one cohesive linear sequence: payload destructuring, engine input construction, the walk, and the outcome mapping"
)]
pub(crate) async fn run_one_arm(payload: ArmPayload) -> Result<(usize, LuaFanoutResult)> {
    let ArmPayload {
        worker,
        item,
        index,
        store,
        client,
        args,
        execution,
        when,
        last_reply,
        shared,
        bindings,
        models,
        analysis,
        shared_tools,
        max_tool_iterations,
        limits,
        parent_id,
        section_count,
        turns,
        observer,
        debug,
        cancel,
    } = payload;

    let taskid = (index + 1).to_string();
    // Erase the proxy handle once, up front: the finalizer, the persistent
    // host APIs, and the direct observation calls all share this one handle.
    let observer_dyn: Arc<dyn Observer> = observer;
    let observer: &dyn Observer = observer_dyn.as_ref();
    observer.observe(&execution, &worker.name, detail::FANOUT_ARM_STARTED);

    // The guard defaults to a CANCELLED terminal event; the epilogue below
    // upgrades it to the arm's real outcome unless the arm is aborted first.
    let mut finalizer = ArmFinalizer::new(
        Arc::clone(&observer_dyn),
        execution.clone(),
        worker.name.clone(),
    );

    let mut vm =
        match SectionVm::new_for_section(&bindings, &models, &execution, observer, &worker.name) {
            Ok(vm) => vm,
            Err(error) => {
                finalizer.finish(detail::FANOUT_ARM_FAILED);
                return Err(error);
            }
        };

    // The body performs no teardown; every fallible step uses `?`. It returns the
    // arm result paired with its distinct terminal event.
    let body = async {
        vm.apply_lua_limits(limits.lua_memory().get(), limits.lua_logs().get())?;
        let now = now_rfc3339_checked()?;
        let mut sys = sys_json(
            &when,
            &now,
            parent_id,
            &worker.name,
            &execution,
            section_count,
        );
        // The arm's own sys extra: the 1-based collection position.
        sys["taskid"] = Value::String(taskid);

        // The arm runs the walk's shared VM setup with its own deltas: the
        // `sys` extras (`taskid`, the parent `id`), the `item` seed, and stub
        // control globals. Nested execute/fanout/list_from_section have no
        // walk to re-enter here, so they fail loudly. `jump` records into the
        // arm VM's slot and is rejected at the outcome mapping below.
        let setup = SectionVmSetup {
            args: &args,
            sys: &sys,
            store: &store,
            last_reply: last_reply.as_deref(),
            seed: VmSeed::Item(&item),
            observer_arc: &observer_dyn,
            section_name: &worker.name,
            task_handles: &[],
            shared: &shared,
        };
        setup_section_vm(
            &mut vm,
            &setup,
            |_, _| {
                Err(Error::Lua(
                    "execute() is not available inside a fanout arm".to_owned(),
                ))
            },
            |_, _| {
                Err(Error::Lua(
                    "fanout() is not available inside a fanout arm".to_owned(),
                ))
            },
            |_| {
                Err(Error::Lua(
                    "list_from_section() is not available inside a fanout arm".to_owned(),
                ))
            },
        )?;

        // The infer hook carries a lazy client source: a nested `model:infer`
        // surfaces a concrete construction error on first use instead of the
        // setup swallowing it.
        attach_engine_infer_hook(
            &vm,
            client.clone(),
            limits,
            &shared_tools,
            Arc::clone(&observer_dyn),
            debug.clone(),
            &execution,
            &worker.name,
            max_tool_iterations,
            &turns,
            &analysis,
        );

        // The shared block walk: every Lua and prose block in order, the
        // conversation and reply rolling forward, the tool scope rebuilt per
        // prose block, and a gateway client created from the environment when
        // the arm was handed none. The arm's only walk-input delta is `item`.
        let walk_ctx = BlockWalkContext {
            args: &args,
            execution: &execution,
            observer,
            debug: debug.as_deref(),
            bindings: &bindings,
            models: &models,
            analysis: &analysis,
            shared_tools: &shared_tools,
            max_tool_iterations,
            limits,
            turns: turns.as_ref(),
            item: Some(&item),
        };
        let mut client = client;
        match walk_section_blocks(
            &mut vm,
            &walk_ctx,
            &worker,
            sys,
            last_reply.as_deref(),
            &mut client,
        )
        .await
        {
            Ok(SectionFlow::Returned(value)) => Ok((
                LuaFanoutResult::success(item, value),
                detail::FANOUT_ARM_SUCCEEDED,
            )),
            Ok(SectionFlow::FellThrough { reply }) => Ok((
                LuaFanoutResult::success(item, reply.unwrap_or_default()),
                detail::FANOUT_ARM_SUCCEEDED,
            )),
            Ok(SectionFlow::Jumped { heading, .. }) => Err(Error::Lua(format!(
                "jump({heading}) is not allowed inside a fanout arm"
            ))),
            // One stuck arm must not kill sibling evidence facets.
            Err(Error::ToolLoopExhausted) => {
                let stub = format!(
                    "## {}\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)",
                    subst::render_item(&item)
                );
                Ok((
                    LuaFanoutResult::exhausted_stub(item, stub),
                    detail::FANOUT_ARM_EXHAUSTED,
                ))
            }
            Err(error) => Err(error),
        }
    };

    // Re-install the explicit cancel handle on THIS arm's task so its Lua
    // instruction hook and tool loop observe cancellation cooperatively; a
    // spawned task never inherits the parent's task-local (PF-CANCEL-002).
    let outcome: Result<(LuaFanoutResult, Observation)> = cancel::maybe_scope(cancel, body).await;

    // Single epilogue: tear the VM down once, then record exactly one terminal
    // observation matching the arm's real outcome.
    vm.teardown(observer, &worker.name);
    match outcome {
        Ok((result, event)) => {
            finalizer.finish(event);
            Ok((index, result))
        }
        Err(error) => {
            finalizer.finish(detail::FANOUT_ARM_FAILED);
            Err(error)
        }
    }
}
