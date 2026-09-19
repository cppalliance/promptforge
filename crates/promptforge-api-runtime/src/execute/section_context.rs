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
//! [`SectionContext::new_fanout_arm`] for a fanout arm; the three live in
//! the `construct` sibling), the scheduler's chain steps run the blocks,
//! and the frame's [`Drop`] impl is the single teardown boundary.
//!
//! The run-scoped inputs
//! (bindings, models, limits, the shared tools) arrive through the
//! [`RunState`].

#[path = "section_context-construct.rs"]
mod construct;

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::debug::DebugCapture;
use crate::lua::{ProseState, SectionVm, ToolBinding, ToolCallCounts};
use crate::observe::{Observer, detail};
use crate::{Error, Result, subst};

use super::context::RunState;

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

/// The frame's effective reporting handles for a model round: the
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
            return Err(Error::internal(
                "the section frame's VM lives until the frame's own drop",
            ));
        };
        self.var = vm.var()?;
        Ok(self.var.clone())
    }

    /// Reads the H1 pass's `argv` back as JSON while the frame is live: the
    /// value the walk's sections inherit frozen - the derived parse, or H1's
    /// repair. `None` reads as nil.
    ///
    /// # Errors
    /// Returns [`Error::Lua`](crate::Error::Lua) when H1 left `argv` as
    /// non-JSON data, or [`Error::Internal`] if the VM is gone.
    pub(crate) fn read_argv(&self) -> Result<Option<serde_json::Value>> {
        self.vm()?.argv_json().map_err(Error::from)
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
        self.vm.as_ref().ok_or(Error::internal(
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
    pub(crate) fn install_lazy_prose(&self, ctx: &RunState, template: &str) -> Result<()> {
        let template = template.to_owned();
        let raw_args = ctx.args().to_owned();
        let argv = ctx.argv().cloned();
        let item = self.item.clone();
        self.vm()?
            .install_lazy_prose(move |state: ProseState| -> mlua::Result<String> {
                let globals = |name: &str| (state.globals)(name).map_err(Error::from);
                let sources = subst::Sources {
                    args: &raw_args,
                    argv: argv.as_ref(),
                    item: item.as_ref(),
                    var: &state.var,
                    sys: &state.sys,
                    globals: &globals,
                };
                subst::substitute(&template, &sources).map_err(mlua::Error::external)
            })?;
        Ok(())
    }

    /// The frame's effective reporting handles for the `chat` rounds the
    /// scheduler applies on this chain (the `models.loop` shim's among
    /// them), each seeded out of the run context (a fanout arm's fork) at
    /// construction.
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
        ctx: &RunState,
        effective: &[ToolBinding],
    ) -> Result<ToolCallCounts> {
        let Self {
            vm, sys, counts, ..
        } = self;
        let Some(vm) = vm.as_ref() else {
            return Err(Error::internal(
                "the section frame's VM lives until the frame's own drop",
            ));
        };
        install_section_scope(vm, ctx, sys, counts, effective)?;
        let counts = counts
            .as_ref()
            .ok_or(Error::internal("the scope install seeds the counts"))?;
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
    ctx: &RunState,
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
