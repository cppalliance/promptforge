//! The per-section frame: one section entry's owned state within a run.
//!
//! [`SectionContext`] is born at a section entry and dies at its teardown.
//! It owns the section VM plus the state the block walk reads and writes -
//! the `sys` JSON, the seeded `var`, the fanout arm's item, and the
//! tool-call counts - and it
//! carries the frame's effective reporting handles (observer, debug sink,
//! turn counter) seeded out of the run context; a fanout arm's context is
//! the fanout's fork, so the handles reach the frame and the arm's nested
//! chains through the one value. Each driver is one
//! construct-run-teardown cycle: the constructor absorbs the VM
//! construction and setup preamble ([`SectionContext::new`] for a walked
//! section, [`SectionContext::new_live_h1`] for the live H1 pass,
//! [`SectionContext::new_fanout_arm`] for a fanout arm), the scheduler's
//! chain steps run the blocks, and the frame's [`Drop`] impl is the single
//! teardown boundary.
//!
//! The run-scoped inputs
//! (bindings, models, limits, the shared tools) arrive through the
//! [`RunContext`].

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::debug::DebugCapture;
use crate::lua::{ProseState, SectionVm, ToolBinding, ToolCallCounts, install_live_h1_shim_base};
use crate::observe::{Observer, detail};
use crate::parser::Section;
use crate::store::Access;
use crate::{Error, Result, subst};

use super::context::RunContext;
use super::engine::{list_items_from_visible, visible_sections};
use super::section_vm::{VmSeed, setup_section_vm};
use super::support::{next_id, now_rfc3339_checked, sys_json};

/// One section entry's owned frame within a run.
///
/// The frame is born at a section entry and dies at its teardown. One
/// section entry is one frame, regardless of arrival mode (fall-through,
/// jump, call); a jump ends the current frame and the driver builds a
/// fresh one for the target - only `var` crosses, as call data.
/// No derives: the VM and the trait-object handles support neither `Clone`
/// nor `Debug`.
pub(crate) struct SectionContext {
    /// The frame's engine: the owned section VM, `Some` from construction
    /// until the frame's `Drop` takes it for the teardown boundary.
    /// `SectionVm` stays a standalone type in `lua/` with its own test
    /// suite - composition, not merger.
    vm: Option<SectionVm>,
    /// The section's own name, retained so `Drop` reports the teardown
    /// boundary and the completion observation without a parameter.
    name: String,
    /// The run's execution id, retained for the completion observation
    /// `Drop` fires on the armed path.
    execution: String,
    /// Armed by [`SectionContext::mark_completed`] on the success path
    /// only, so `Drop` fires `SECTION_FINISHED` on completion (a jump or
    /// return included) and never on an error.
    completed: bool,
    /// The section's `sys` JSON, enriched in place by the walk (the model
    /// binding).
    sys: serde_json::Value,
    /// The walk's clipboard: seeded into the VM at construction, read back
    /// out of it before teardown so the walk rolls it forward.
    var: serde_json::Value,
    /// The fanout arm's collection member for `{{ item }}` substitution;
    /// `None` outside an arm, so always `None` on the walk.
    item: Option<serde_json::Value>,
    /// The per-section tool-call counts, installed at the first
    /// script-initiated `tools.call`.
    counts: Option<ToolCallCounts>,
    /// The frame's effective observer handle: the run's own on the walk, a
    /// fanout arm's proxy in a fanout.
    observer: Arc<dyn Observer>,
    /// Opt-in raw request/response capture for each model turn.
    debug: Option<Arc<dyn DebugCapture>>,
    /// The model-turn counter this frame advances.
    turns: Arc<AtomicU32>,
}

/// The frame's effective reporting handles for the model tool loop: the
/// observer, the opt-in debug capture sink, and the model-turn counter.
pub(crate) struct ReportingHandles {
    /// The frame's effective observer handle.
    pub(crate) observer: Arc<dyn Observer>,
    /// The frame's opt-in raw request/response capture sink.
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    /// The model-turn counter the frame advances.
    pub(crate) turns: Arc<AtomicU32>,
}

impl SectionContext {
    /// Constructs the frame for one walked section and runs its setup
    /// preamble: the `sys` JSON, the section-started observation, VM
    /// construction and limits, the control surface (the `jump` and
    /// `list_from_section` callbacks resolved over the section's visible
    /// set, plus the coroutine yield shims for the suspending calls), the
    /// shared setup half (host injection, host APIs, the shared replay, the
    /// captured alias bindings).
    ///
    /// `siblings` is the caller's own walk slice, from which the section's
    /// visible set (its siblings minus itself, plus its direct children) is
    /// built for the `list_from_section` callback. `section_id` is the
    /// section's `sys.id`: the next value from the run-global counter.
    /// `var` is the walk's current clipboard, seeded into the
    /// section's VM.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction or limits failure propagates bare, before any teardown
    /// observation exists; a setup failure tears the fresh VM down first, so
    /// the teardown boundary still fires exactly once on that path.
    pub(crate) fn new(
        ctx: &RunContext,
        access: &Arc<Access>,
        section: &Section,
        siblings: &[Section],
        section_id: u64,
        var: &serde_json::Value,
    ) -> Result<Self> {
        let sys = ctx.sys_json(section_id, section.name())?;
        ctx.observer()
            .observe(ctx.execution(), section.name(), detail::SECTION_STARTED);
        let tool_set = ctx.tool_set_snapshot()?;
        let model_set = ctx.model_set_snapshot()?;
        let mut vm = SectionVm::new_for_section(
            ctx.nonce(),
            &tool_set,
            &model_set,
            ctx.execution(),
            ctx.observer().as_ref(),
            section.name(),
        )?;
        // A limits failure propagates bare: no teardown runs here, so no
        // LUA_TEARDOWN_* observation fires on this path.
        vm.apply_lua_limits(
            ctx.limits().lua_memory().get(),
            ctx.limits().lua_logs().get(),
        )?;
        // The `list_from_section` callback resolves over the section's
        // visible set; the suspending calls (`call`, `fanout`,
        // `models.infer`) are the yield shims the setup half installs.
        let visible = visible_sections(siblings, section);
        let list_callback = move |heading: String| list_items_from_visible(&heading, &visible);
        // The setup half of the section lifecycle - host injection, host
        // APIs, the control surface, the shared replay, and the captured
        // alias bindings - is shared with the fanout arm; only the seed, the
        // `sys` extras, and the callback's visible set are the walk's own.
        let setup = ctx.vm_setup(
            &sys,
            VmSeed {
                var: Some(var),
                item: None,
            },
            access,
            section.name(),
        );
        // Setup runs on the bare VM so a failure tears it down here: the
        // frame does not exist yet, so its `Drop` cannot own this path.
        if let Err(error) = setup_section_vm(&mut vm, &setup, list_callback) {
            vm.teardown(ctx.observer().as_ref(), section.name());
            return Err(error);
        }
        Ok(Self {
            vm: Some(vm),
            name: section.name().to_owned(),
            execution: ctx.execution().to_owned(),
            completed: false,
            sys,
            var: var.clone(),
            item: None,
            counts: None,
            observer: Arc::clone(ctx.observer()),
            debug: ctx.debug().cloned(),
            turns: Arc::clone(ctx.turns()),
        })
    }

    /// Constructs the frame for the live H1 pass and runs its setup
    /// preamble: the `sys` JSON (id 0 under the prompt's title), VM
    /// construction and limits, host injection, the host APIs, the H1
    /// control-global stubs, and the live H1 shim base.
    ///
    /// H1 is the level-1 section: it runs first and is never re-entered, so
    /// the frame seeds an empty `var` and no item. The scheduler answers the pass's `models.infer` yields (with
    /// or without a leading handle) through its driver, so the shim base
    /// keeps the control stubs,
    /// which raise before anything structural can yield.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction or limits failure propagates bare, before any teardown
    /// observation exists; a setup failure tears the fresh VM down first, so
    /// the teardown boundary still fires exactly once on that path.
    pub(crate) fn new_live_h1(ctx: &RunContext, access: &Arc<Access>) -> Result<Self> {
        let title = ctx.prompt().title();
        let now = now_rfc3339_checked()?;
        let sys = sys_json(
            &now,
            &now,
            0,
            title,
            ctx.execution(),
            ctx.prompt().sections().len(),
        );
        let mut vm = SectionVm::new(ctx.nonce(), ctx.execution(), ctx.observer().as_ref(), title)?;
        // A limits failure propagates bare: no teardown runs here, so no
        // LUA_TEARDOWN_* observation fires on this path.
        vm.apply_lua_limits(
            ctx.limits().lua_memory().get(),
            ctx.limits().lua_logs().get(),
        )?;
        // Setup runs on the bare VM so a failure tears it down here: the
        // frame does not exist yet, so its `Drop` cannot own this path.
        if let Err(error) = setup_live_h1(&mut vm, ctx, access, &sys, title)
            .and_then(|()| install_live_h1_shim_base(vm.lua()).map_err(Error::from))
        {
            vm.teardown(ctx.observer().as_ref(), title);
            return Err(error);
        }
        Ok(Self {
            vm: Some(vm),
            name: title.to_owned(),
            execution: ctx.execution().to_owned(),
            completed: false,
            sys,
            var: serde_json::json!({}),
            item: None,
            counts: None,
            observer: Arc::clone(ctx.observer()),
            debug: ctx.debug().cloned(),
            turns: Arc::clone(ctx.turns()),
        })
    }

    /// Constructs the frame for one fanout arm and runs its setup preamble:
    /// VM construction and limits, the `sys` JSON carrying the arm's
    /// run-global `id` and its 1-based per-fanout `index`, the control
    /// surface (the `list_from_section` callback resolved over the worker's
    /// visible set: its home slice plus its children; plus the yield
    /// shims), and the shared setup half.
    ///
    /// The seed is the fanout's own: the collection `item`, the arm's
    /// spawned access capability (its claims-model identity), and the
    /// caller's cloned `var`. The
    /// effective reporting handles
    /// are the fanout's too: the run's own observer and debug sink with the
    /// fanout's fresh turn counter arrive through the context's fanout fork,
    /// so the arm's nested `call`/`fanout` chains report through them as
    /// well.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction failure propagates bare - no VM exists to tear down. A
    /// limits, `sys`, or setup failure tears the fresh VM down once here:
    /// the chain owns the run phase's teardown boundary, so the
    /// construction phase keeps its own and every path tears down exactly
    /// once.
    pub(crate) fn new_fanout_arm(
        ctx: &RunContext,
        access: &Arc<Access>,
        worker: &Section,
        home: &[Section],
        index: usize,
        item: serde_json::Value,
        var: &serde_json::Value,
    ) -> Result<Self> {
        let tool_set = ctx.tool_set_snapshot()?;
        let model_set = ctx.model_set_snapshot()?;
        let mut vm = SectionVm::new_for_section(
            ctx.nonce(),
            &tool_set,
            &model_set,
            ctx.execution(),
            ctx.observer().as_ref(),
            worker.name(),
        )?;
        // The limits install and the `sys` build are the construction
        // phase's fallible steps once the VM exists; a failure tears the
        // fresh VM down once here, matching the single teardown the arm's
        // epilogue owns for the run phase.
        let sys = match vm
            .apply_lua_limits(
                ctx.limits().lua_memory().get(),
                ctx.limits().lua_logs().get(),
            )
            .map_err(Error::from)
            .and_then(|()| {
                let mut sys = ctx.sys_json(next_id(ctx.ids()), worker.name())?;
                // The arm's own sys extra: its 1-based position within this
                // fanout. Absent outside a fanout, so a walked section
                // reading `sys.index` raises the sealed-sys unknown-field
                // error; a nested fanout's arms restart at 1.
                sys["index"] = serde_json::Value::from(index + 1);
                Ok(sys)
            }) {
            Ok(sys) => sys,
            Err(error) => {
                vm.teardown(ctx.observer().as_ref(), worker.name());
                return Err(error);
            }
        };
        let item = Some(item);
        // The `list_from_section` callback resolves over the worker's
        // visible set (its home slice plus its children); the suspending
        // calls are the yield shims the setup half installs.
        let visible = visible_sections(home, worker);
        let list_callback = move |heading: String| list_items_from_visible(&heading, &visible);
        // The setup half is shared with the walk; only the seed, the `sys`
        // extra, and the callback's visible set are the arm's own.
        let setup = ctx.vm_setup(
            &sys,
            VmSeed {
                var: Some(var),
                item: item.as_ref(),
            },
            access,
            worker.name(),
        );
        // Setup runs on the bare VM so a failure tears it down here: the
        // frame does not exist yet, so its `Drop` cannot own this path.
        if let Err(error) = setup_section_vm(&mut vm, &setup, list_callback) {
            vm.teardown(ctx.observer().as_ref(), worker.name());
            return Err(error);
        }
        Ok(Self {
            vm: Some(vm),
            name: worker.name().to_owned(),
            execution: ctx.execution().to_owned(),
            completed: false,
            sys,
            var: var.clone(),
            item,
            counts: None,
            observer: Arc::clone(ctx.observer()),
            debug: ctx.debug().cloned(),
            turns: Arc::clone(ctx.turns()),
        })
    }

    /// Reads the section's final `var` back into the frame and returns it,
    /// so the walk rolls it forward. Must run while the frame is live,
    /// before its drop: the read goes through the live VM.
    ///
    /// # Errors
    /// Returns [`Error::Lua`](crate::Error::Lua) if the VM's `var` cannot be
    /// converted back to JSON (the write guard keeps this conversion from
    /// failing in practice).
    pub(crate) fn read_var(&mut self) -> Result<serde_json::Value> {
        let Some(vm) = self.vm.as_mut() else {
            return Err(Error::Internal(
                "the section frame's VM lives until the frame's own drop",
            ));
        };
        self.var = vm.var()?;
        Ok(self.var.clone())
    }

    /// Arms the completion flag: the block walk completed (a jump or
    /// return included) and the final `var` is read back, so the frame's
    /// drop fires `SECTION_FINISHED` after the teardown pair. No error
    /// path arms it, so an error never fires `SECTION_FINISHED`.
    pub(crate) fn mark_completed(&mut self) {
        self.completed = true;
    }

    /// Borrows the frame's VM for the scheduler's coroutine driving (block
    /// start, yield validation, answer resume) and model resolution.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the VM is gone, which only the frame's
    /// own drop does - a live frame always holds it.
    pub(crate) fn vm(&self) -> Result<&SectionVm> {
        self.vm.as_ref().ok_or(Error::Internal(
            "the section frame's VM lives until the frame's own drop",
        ))
    }

    /// Installs the pending Markdown buffer as the VM's fresh read-only
    /// lazy `prose` template before one Lua coroutine starts. The first
    /// runtime read snapshots the section state, renders every `{{ }}`
    /// substitution once, and memoizes the result; the buffer is the
    /// scheduler's pending prose, empty when no Markdown accumulated.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the guard cannot be installed, or
    /// [`Error::Internal`] if the VM is gone.
    pub(crate) fn install_lazy_prose(&self, ctx: &RunContext, template: &str) -> Result<()> {
        let template = template.to_owned();
        let args = ctx.args().to_owned();
        let item = self.item.clone();
        self.vm()?
            .install_lazy_prose(move |state: ProseState| -> mlua::Result<String> {
                let globals = |name: &str| (state.globals)(name).map_err(Error::from);
                subst::substitute(
                    &template,
                    &args,
                    item.as_ref(),
                    &state.var,
                    &state.sys,
                    &globals,
                )
                .map_err(mlua::Error::external)
            })?;
        Ok(())
    }

    /// The frame's effective reporting handles for the model tool loop a
    /// `models.loop` dispatch runs, each seeded out of the run context (a
    /// fanout arm's fork) at construction.
    pub(crate) fn reporting_handles(&self) -> ReportingHandles {
        ReportingHandles {
            observer: Arc::clone(&self.observer),
            debug: self.debug.clone(),
            turns: Arc::clone(&self.turns),
        }
    }

    /// The frame's tool-call counts for a script-initiated dispatch,
    /// running the same one-time scope install the first prose block
    /// performs (the counts and the Lua `tools.calls` table, the model
    /// freeze, the `sys.model` enrichment), then seeding any alias the
    /// effective scope has gained since. The returned handle shares the
    /// installed counts, so the dispatch task increments them off the
    /// driver thread.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of the scope install or the
    /// alias seeding.
    pub(crate) fn script_call_counts(
        &mut self,
        ctx: &RunContext,
        effective: &[ToolBinding],
    ) -> Result<ToolCallCounts> {
        let Self {
            vm, sys, counts, ..
        } = self;
        let Some(vm) = vm.as_ref() else {
            return Err(Error::Internal(
                "the section frame's VM lives until the frame's own drop",
            ));
        };
        install_section_scope(vm, ctx, sys, counts, effective)?;
        let counts = counts
            .as_ref()
            .ok_or(Error::Internal("the scope install seeds the counts"))?;
        for binding in effective {
            counts.ensure(binding.alias())?;
        }
        Ok(counts.clone())
    }
}

/// Installs the section's one-time tool-call counts and model resolution,
/// gated on the counts slot: the first consumer - the section's first
/// script-initiated `tools.call` - performs the install, and every later
/// call is a no-op. The counts install backs the Lua `tools.calls` table;
/// the model resolution freezes the section's binding and enriches
/// `sys.model`.
///
/// # Errors
/// Returns the [`Error`] of the counts install, the model resolution, or
/// the `sys` re-seal.
fn install_section_scope(
    vm: &SectionVm,
    ctx: &RunContext,
    sys: &mut serde_json::Value,
    counts: &mut Option<ToolCallCounts>,
    effective_bindings: &[ToolBinding],
) -> Result<()> {
    if counts.is_some() {
        return Ok(());
    }
    *counts = Some(vm.install_tool_call_counts(effective_bindings)?);
    let resolved_model = crate::lua::resolve_model_binding(ctx.models(), &vm.model_runtime)?;
    if let Some(binding) = resolved_model.as_ref() {
        let current = vm.current_sys(sys)?;
        let enriched = crate::lua::enrich_sys_model(&current, binding);
        vm.re_seal_sys(&enriched)?;
        *sys = enriched;
    }
    Ok(())
}

/// The fallible setup half of the live H1 lifecycle: host injection, the
/// host APIs, and the control-global stubs. One function, so the
/// constructor's single teardown-on-error branch covers every step.
///
/// # Errors
/// Returns the [`Error`](crate::Error) of whichever step failed.
fn setup_live_h1(
    vm: &mut SectionVm,
    ctx: &RunContext,
    access: &Arc<Access>,
    sys: &serde_json::Value,
    title: &str,
) -> Result<()> {
    vm.inject_host(ctx.args(), sys, access)?;
    vm.install_host_apis(ctx.observer(), title)?;
    vm.install_h1_control_stubs().map_err(Error::from)
}

impl Drop for SectionContext {
    fn drop(&mut self) {
        // The single teardown boundary: every exit path - success, error,
        // or early return - drops the frame, so the VM tears down exactly
        // once here. `SECTION_FINISHED` follows only on the armed
        // (completed) path; an error reports the teardown pair alone. The
        // `let-else` is defensive: `Drop` runs once, so the VM is always
        // here, and the destructor stays infallible.
        let Some(vm) = self.vm.take() else {
            return;
        };
        vm.teardown(self.observer.as_ref(), &self.name);
        if self.completed {
            self.observer
                .observe(&self.execution, &self.name, detail::SECTION_FINISHED);
        }
    }
}
