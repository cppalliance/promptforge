//! Execution of a single fanout arm: its owned payload, the terminal-observation
//! guard, and the thin adapter over the shared engine.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use serde_json::Value;

use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::execute::{
    BlockRunMode, ControlContext, RunFrame, SectionFlow, VmSeed, make_control_globals, next_id,
    run_one_section_impl, setup_section_vm,
};
use crate::lua::{LuaFanoutResult, SectionVm};
use crate::observe::{Observation, Observer, detail};
use crate::parser::Section;
use crate::store::WriteScope;
use crate::{Error, Result, cancel, subst};

use super::FanoutContext;
use super::proxies::ProxyObserver;

/// The inputs every arm of one fanout shares, carried by `Arc` into each
/// arm's payload.
///
/// Built once per `run_fanout_arms` call so the per-arm spawn copies one
/// `Arc` instead of deep-cloning the worker subtree and the home slice.
pub(crate) struct ArmInputs {
    /// The owned run context the arm's control globals capture, shared by
    /// every arm of the fanout.
    pub(crate) control: Arc<ControlContext>,
    pub(crate) worker: Section,
    /// The worker's home slice - the set it was resolved from, minus the
    /// worker - built by the fanout callback where the layout is constructed.
    /// The arm's control globals derive their resolution set from it (the
    /// home slice plus the worker's children); the arm never inverts the
    /// engine's visible-set construction.
    pub(crate) home: Vec<Section>,
    /// The client handed to the fanout, if any: the arm's walk and its nested
    /// chains start from it, creating one lazily when absent.
    pub(crate) client: Option<GatewayClient>,
    pub(crate) last_reply: Option<String>,
    /// The fanout caller's `var` snapshot; each arm seeds its VM from a
    /// clone of it, and the arm's writes never reach the caller.
    pub(crate) var: Value,
    /// This fanout's store-write token: one per `run_fanout_arms` call, so
    /// the store's write registry can tell two arms of THIS fanout (a
    /// write-write race) from a later fanout's write (legal).
    pub(crate) write_token: u64,
    /// The arm's execute depth: the fanout caller's depth plus one, so
    /// recursion accounting accumulates across the fanout boundary.
    pub(crate) execute_depth: usize,
    /// Explicit cancellation handle carried across the spawn boundary, since a
    /// spawned arm does not inherit the parent task-local (PF-CANCEL-002).
    pub(crate) cancel: Option<cancel::CancelHandle>,
}

impl ArmInputs {
    /// Builds the shared arm inputs from the borrowed fanout context, so the
    /// field copy lives in exactly one place. Each arm runs one execute level
    /// deeper than the fanout caller, so recursion accounting accumulates
    /// across the fanout boundary instead of resetting.
    pub(crate) fn from_context(
        ctx: &FanoutContext<'_>,
        worker: &Section,
        turns: &Arc<AtomicU32>,
        observer: Arc<ProxyObserver>,
        debug: Option<Arc<dyn DebugCapture>>,
        cancel: Option<cancel::CancelHandle>,
    ) -> Self {
        Self {
            control: Arc::new(ControlContext::from_fanout(ctx, turns, observer, debug)),
            worker: worker.clone(),
            home: ctx.home.to_vec(),
            client: ctx.client.clone(),
            last_reply: ctx.last_reply.map(str::to_owned),
            var: ctx.var.clone(),
            write_token: ctx.store.next_write_token(),
            execute_depth: ctx.execute_depth + 1,
            cancel,
        }
    }
}

/// Everything one spawned fanout arm owns for its independent execution: the
/// shared inputs plus the arm's own collection member and position.
pub(crate) struct ArmPayload {
    pub(crate) inputs: Arc<ArmInputs>,
    pub(crate) item: Value,
    pub(crate) index: usize,
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

/// Test-only fault-injection sentinel: an arm whose item is this exact
/// string panics on entry, so the `FanoutArmJoin` mapping is covered through
/// the real `run_fanout_arms` select loop.
#[cfg(test)]
pub(crate) const PANIC_ARM_SENTINEL: &str = "__fanout_test_panic_arm__";

/// Test-only fault-injection sentinel: an arm whose worker carries this
/// exact section name fails VM construction, so the finalizer's
/// construction-failure branch is covered through the real driver path. No
/// production input can fail construction on cue - the failure modes (Lua
/// init, host injection) are not name-keyed.
#[cfg(test)]
pub(crate) const FAIL_ARM_VM_SENTINEL: &str = "__fanout_test_fail_arm_vm__";

/// Runs one fanout arm to completion.
///
/// The arm is a thin adapter over the shared engine: the body builds the
/// engine's inputs from the payload (the `sys` JSON carrying the arm's
/// run-global `id` and its per-fanout `index`, the caller's cloned `var`,
/// the `item` seed, the
/// proxy observer, the shared turns counter), installs the engine's real
/// control globals resolved over the worker's visible set
/// ([`make_control_globals`]), runs the setup half
/// ([`setup_section_vm`]), installs the `model:infer` hook with a lazy client
/// source, and drives the shared block walk ([`run_one_section_impl`]).
///
/// A [`SectionFlow::Jumped`] transfers control rather than erroring: the
/// arm's remaining blocks are skipped and a child walk runs from the target
/// under the engine's chain-slice rule (a child target walks the worker's
/// children; any other visible target walks the worker's home slice),
/// each entry taking the next run-global `sys.id` like a contained
/// `execute` chain. The
/// arm's result text is the child walk's returned value or final reply.
/// [`SectionFlow::Returned`] and [`SectionFlow::FellThrough`] map to
/// [`LuaFanoutResult::success`]; [`Error::ToolLoopExhausted`] soft-degrades to
/// the incomplete stub so one stuck arm cannot kill sibling evidence.
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
        inputs,
        item,
        index,
    } = payload;
    // Test-only fault injection: an arm handed the sentinel item panics on
    // entry, so a test can drive a genuine non-cancellation `JoinError`
    // through the `run_fanout_arms` select loop. No production input can
    // panic an arm - every arm-internal failure is a `Result`.
    #[cfg(test)]
    assert!(
        item.as_str() != Some(PANIC_ARM_SENTINEL),
        "test-injected arm panic"
    );
    let control = &inputs.control;
    let worker = &inputs.worker;

    // The proxy handle inside the shared control context is the one observer
    // handle the finalizer, the persistent host APIs, and the direct
    // observation calls all share.
    let observer: &dyn Observer = control.observer.as_ref();
    observer.observe(&control.execution, &worker.name, detail::FANOUT_ARM_STARTED);

    // The guard defaults to a CANCELLED terminal event; the epilogue below
    // upgrades it to the arm's real outcome unless the arm is aborted first.
    let mut finalizer = ArmFinalizer::new(
        Arc::clone(&control.observer),
        control.execution.clone(),
        worker.name.clone(),
    );

    let constructed = SectionVm::new_for_section(
        &control.bindings,
        &control.models,
        &control.execution,
        observer,
        &worker.name,
    );
    // Test-only fault injection: an arm whose worker is named the sentinel
    // fails VM construction here, driving the finalizer's
    // construction-failure branch (finish FAILED, return before the body).
    #[cfg(test)]
    let constructed = if worker.name == FAIL_ARM_VM_SENTINEL {
        Err(Error::Internal("test-injected VM construction failure"))
    } else {
        constructed
    };
    let mut vm = match constructed {
        Ok(vm) => vm,
        Err(error) => {
            finalizer.finish(detail::FANOUT_ARM_FAILED);
            return Err(error);
        }
    };

    // The body performs no teardown; every fallible step uses `?`. It returns the
    // arm result paired with its distinct terminal event.
    let body = async {
        vm.apply_lua_limits(
            control.limits.lua_memory().get(),
            control.limits.lua_logs().get(),
        )?;
        let mut sys = control.sys_json(next_id(&control.ids), &worker.name)?;
        // The arm's own sys extra: its 1-based position within this fanout.
        // Absent outside a fanout, so a walked section reading `sys.index`
        // raises the sealed-sys unknown-field error; a nested fanout's arms
        // restart at 1.
        sys["index"] = Value::from(index + 1);

        // The arm installs the engine's real control globals: `execute`,
        // `fanout`, and `list_from_section` resolve over the worker's visible
        // set (its home slice plus its children), and the arm's execute depth
        // keeps recursion accounting accumulating across the fanout boundary.
        let (execute_callback, fanout_callback, list_callback) = make_control_globals(
            control,
            &inputs.client,
            worker.clone(),
            inputs.home.clone(),
            inputs.execute_depth,
            inputs.last_reply.clone(),
        );

        // The arm runs the walk's shared VM setup with its own deltas: the
        // `sys` extra (`index`), the caller's cloned
        // `var`, the `item` seed, and its store-write scope (this fanout's
        // token plus the arm's 1-based index, matching `sys.index`).
        let setup = control.vm_setup(
            &sys,
            inputs.last_reply.as_deref(),
            VmSeed {
                var: Some(&inputs.var),
                item: Some(&item),
            },
            Some(WriteScope::new(inputs.write_token, index + 1)),
            &worker.name,
        );
        setup_section_vm(
            &mut vm,
            &setup,
            execute_callback,
            fanout_callback,
            list_callback,
        )?;

        // The infer hook carries a lazy client source: a nested `model:infer`
        // surfaces a concrete construction error on first use instead of the
        // setup swallowing it.
        control.attach_infer_hook(&vm, inputs.client.clone(), &worker.name);

        // The shared block walk: every Lua and prose block in order, the
        // conversation and reply rolling forward, the tool scope rebuilt per
        // prose block, and a gateway client created from the environment when
        // the arm was handed none. The arm's only frame delta is `item`.
        let frame = RunFrame {
            item: Some(&item),
            ..control.walk_context(&control.args)
        };
        let mut client = inputs.client.clone();
        match run_one_section_impl(
            &mut vm,
            &frame,
            &worker.name,
            &worker.blocks,
            BlockRunMode::Section,
            sys,
            inputs.last_reply.as_deref(),
            &mut client,
        )
        .await
        {
            Ok(flow) => {
                let text = match flow {
                    SectionFlow::Returned(value) => value,
                    SectionFlow::FellThrough { reply } => reply.unwrap_or_default(),
                    // A jump transfers control; it does not return. The arm's
                    // own remaining blocks are skipped, and a child walk runs
                    // from the target under the engine's chain-slice rule,
                    // each entry taking the next run-global `sys.id` like a
                    // contained
                    // execute chain. The child walk shares the arm's var,
                    // read back at the jump. The arm's result text is the
                    // child walk's returned value or final reply.
                    SectionFlow::Jumped { heading, reply } => {
                        let var = vm.var()?;
                        control
                            .drive_contained_chain(
                                worker,
                                &inputs.home,
                                &heading,
                                &control.args,
                                reply,
                                inputs.execute_depth,
                                &mut client,
                                var,
                            )
                            .await?
                    }
                };
                Ok((
                    LuaFanoutResult::success(item, text),
                    detail::FANOUT_ARM_SUCCEEDED,
                ))
            }
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
    let outcome: Result<(LuaFanoutResult, Observation)> =
        cancel::maybe_scope(inputs.cancel.clone(), body).await;

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
