//! The per-section frame: one section entry's owned state within a run.
//!
//! [`SectionContext`] is born at a section entry and dies at its teardown.
//! It owns the section VM plus the state the block walk reads and writes -
//! the `sys` JSON, the seeded `var`, the rolling reply, the conversation,
//! the tool-call counts, and the resolved completion options - and it
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

use crate::client::{GatewayClient, Message};
use crate::debug::DebugCapture;
use crate::lua::{SectionVm, ToolBinding, ToolCallCounts, install_live_h1_shim_base};
use crate::model::CompletionOptions;
use crate::observe::{Observer, detail};
use crate::parser::Section;
use crate::store::WriteScope;
use crate::{Error, Result};

use super::block_walk::{install_section_scope, run_live_h1_prose, run_section_prose};
use super::context::RunContext;
use super::engine::{list_items_from_visible, visible_sections};
use super::section_vm::{VmSeed, setup_section_vm};
use super::support::{next_id, now_rfc3339_checked, sys_json};
use super::tool_loop::ProseMode;

/// One section entry's owned frame within a run.
///
/// The frame is born at a section entry and dies at its teardown. One
/// section entry is one frame, regardless of arrival mode (fall-through,
/// jump, execute); a jump ends the current frame and the driver builds a
/// fresh one for the target - only `reply` and `var` cross, as call data.
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
    /// binding, each outcome's finish reason).
    sys: serde_json::Value,
    /// The walk's clipboard: seeded into the VM at construction, read back
    /// out of it before teardown so the walk rolls it forward.
    var: serde_json::Value,
    /// The fanout arm's collection member for `{{ item }}` substitution;
    /// `None` outside an arm, so always `None` on the walk.
    item: Option<serde_json::Value>,
    /// The reply visible to the walk's prose: seeded from the incoming
    /// reply, rolled forward as prose produces text.
    reply: Option<String>,
    /// The section's conversation, accumulated across prose blocks.
    conversation: Vec<Message>,
    /// The per-section tool-call counts, installed at the first prose block.
    counts: Option<ToolCallCounts>,
    /// The resolved model's per-call fields, set at the first prose block.
    completion_options: Option<CompletionOptions>,
    /// The frame's effective observer handle: the run's own on the walk, a
    /// fanout arm's proxy in a fanout.
    observer: Arc<dyn Observer>,
    /// Opt-in raw request/response capture for each model turn.
    debug: Option<Arc<dyn DebugCapture>>,
    /// The model-turn counter this frame advances.
    turns: Arc<AtomicU32>,
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
    /// `incoming_reply` is the model reply visible to this section's first
    /// prose. `var` is the walk's current clipboard, seeded into the
    /// section's VM.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction or limits failure propagates bare, before any teardown
    /// observation exists; a setup failure tears the fresh VM down first, so
    /// the teardown boundary still fires exactly once on that path.
    pub(crate) fn new(
        ctx: &RunContext,
        section: &Section,
        siblings: &[Section],
        section_id: u64,
        incoming_reply: Option<&str>,
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
        let reply = incoming_reply.map(str::to_owned);
        // The `list_from_section` callback resolves over the section's
        // visible set; the suspending calls (`execute`, `fanout`,
        // `models.infer`) are the yield shims the setup half installs.
        let visible = visible_sections(siblings, section);
        let list_callback = move |heading: String| list_items_from_visible(&heading, &visible);
        // The setup half of the section lifecycle - host injection, host
        // APIs, the control surface, the shared replay, and the captured
        // alias bindings - is shared with the fanout arm; only the seed, the
        // `sys` extras, and the callback's visible set are the walk's own.
        let setup = ctx.vm_setup(
            &sys,
            reply.as_deref(),
            VmSeed {
                var: Some(var),
                item: None,
            },
            // Walk-section store writes are untracked; only fanout arms
            // carry a write scope.
            None,
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
            reply,
            conversation: Vec::new(),
            counts: None,
            completion_options: None,
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
    /// the frame seeds an empty `var`, no reply, no item, and no write
    /// scope. The scheduler answers the pass's `models.infer`/`handle:infer`
    /// yields through its driver, so the shim base keeps the control stubs,
    /// which raise before anything structural can yield.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction or limits failure propagates bare, before any teardown
    /// observation exists; a setup failure tears the fresh VM down first, so
    /// the teardown boundary still fires exactly once on that path.
    pub(crate) fn new_live_h1(ctx: &RunContext) -> Result<Self> {
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
        if let Err(error) = setup_live_h1(&mut vm, ctx, &sys, title)
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
            reply: None,
            conversation: Vec::new(),
            counts: None,
            completion_options: None,
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
    /// The seed is the fanout's own: the collection `item`, the store-write
    /// scope (this fanout's token plus the arm's index, matching
    /// `sys.index`), the caller's cloned `var`, and the caller's reply. The
    /// effective reporting handles
    /// are the fanout's too: the run's own observer and debug sink with the
    /// fanout's fresh turn counter arrive through the context's fanout fork,
    /// so the arm's nested `execute`/`fanout` chains report through them as
    /// well.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction failure propagates bare - no VM exists to tear down. A
    /// limits, `sys`, or setup failure tears the fresh VM down once here:
    /// the chain owns the run phase's teardown boundary, so the
    /// construction phase keeps its own and every path tears down exactly
    /// once.
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame's construction absorbs the arm's whole driver preamble: the fanout's run-context fork, the worker and its home slice, the arm's position, item, write token, and reply and var seeds stay explicit and linear"
    )]
    pub(crate) fn new_fanout_arm(
        ctx: &RunContext,
        worker: &Section,
        home: &[Section],
        index: usize,
        item: serde_json::Value,
        write_token: u64,
        incoming_reply: Option<&str>,
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
        let reply = incoming_reply.map(str::to_owned);
        let item = Some(item);
        // The arm's store-write identity: this fanout's token plus the
        // arm's 1-based index, matching `sys.index`.
        let write_scope = Some(WriteScope::new(write_token, index + 1));
        // The `list_from_section` callback resolves over the worker's
        // visible set (its home slice plus its children); the suspending
        // calls are the yield shims the setup half installs.
        let visible = visible_sections(home, worker);
        let list_callback = move |heading: String| list_items_from_visible(&heading, &visible);
        // The setup half is shared with the walk; only the seed, the `sys`
        // extra, and the callback's visible set are the arm's own.
        let setup = ctx.vm_setup(
            &sys,
            reply.as_deref(),
            VmSeed {
                var: Some(var),
                item: item.as_ref(),
            },
            write_scope,
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
            reply,
            conversation: Vec::new(),
            counts: None,
            completion_options: None,
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

    /// The frame's current reply slot: seeded from the incoming reply,
    /// rolled forward as prose produces text, and synced from the VM's
    /// `reply` global after each of the scheduler's Lua blocks.
    pub(crate) fn reply(&self) -> Option<String> {
        self.reply.clone()
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
            vm,
            sys,
            counts,
            completion_options,
            ..
        } = self;
        let Some(vm) = vm.as_ref() else {
            return Err(Error::Internal(
                "the section frame's VM lives until the frame's own drop",
            ));
        };
        install_section_scope(vm, ctx, sys, counts, completion_options, effective)?;
        let counts = counts
            .as_ref()
            .ok_or(Error::Internal("the scope install seeds the counts"))?;
        for binding in effective {
            counts.ensure(binding.alias())?;
        }
        Ok(counts.clone())
    }

    /// Reads the VM's `reply` global back into the frame's slot and returns
    /// it, so an author's `reply = nil` (or a custom string) steers what the
    /// next prose substitutes and what the chain's finish reports.
    ///
    /// # Errors
    /// Returns [`Error::Lua`](crate::Error::Lua) when `reply` is neither nil
    /// nor a string, or [`Error::Internal`] if the VM is gone.
    pub(crate) fn read_reply(&mut self) -> Result<Option<String>> {
        let reply = self.vm()?.reply()?;
        self.reply.clone_from(&reply);
        Ok(reply)
    }

    /// Runs one prose block through the shared section prose path: the
    /// per-block scope rebuild, substitution, the tool loop, and the
    /// reply/`sys` roll-forward. The scheduler's driver calls this for a
    /// walked section's prose block.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed, as
    /// documented on `run_section_prose`.
    pub(crate) async fn run_prose_block(
        &mut self,
        ctx: &RunContext,
        name: &str,
        text: &str,
        loop_capable: bool,
        client: &mut Option<GatewayClient>,
    ) -> Result<()> {
        let Self {
            vm,
            sys,
            reply,
            conversation,
            counts,
            completion_options,
            item,
            observer,
            debug,
            turns,
            ..
        } = self;
        let Some(vm) = vm.as_mut() else {
            return Err(Error::Internal(
                "the section frame's VM lives until the frame's own drop",
            ));
        };
        let prose_mode = if loop_capable {
            ProseMode::Loop {
                max_tool_iterations: ctx.max_tool_iterations(),
            }
        } else {
            ProseMode::SingleShot
        };
        run_section_prose(
            vm,
            ctx,
            name,
            text,
            prose_mode,
            sys,
            reply,
            conversation,
            counts,
            completion_options,
            item.as_ref(),
            observer.as_ref(),
            debug.as_deref(),
            turns.as_ref(),
            client,
        )
        .await
    }

    /// Runs one live H1 prose block through the shared live prose path:
    /// substitution, the empty-prose skip, the default model and
    /// always-scope read from the bindings-so-far, fresh per-block counts,
    /// and the reply written as a plain global. The scheduler's driver
    /// calls this for the live H1 pass's prose block.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed, as
    /// documented on `run_live_h1_prose`.
    pub(crate) async fn run_live_h1_prose_block(
        &mut self,
        ctx: &RunContext,
        name: &str,
        text: &str,
        loop_capable: bool,
        client: &mut Option<GatewayClient>,
    ) -> Result<()> {
        let Self {
            vm,
            sys,
            reply,
            conversation,
            observer,
            debug,
            turns,
            ..
        } = self;
        let Some(vm) = vm.as_mut() else {
            return Err(Error::Internal(
                "the section frame's VM lives until the frame's own drop",
            ));
        };
        let prose_mode = if loop_capable {
            ProseMode::Loop {
                max_tool_iterations: ctx.max_tool_iterations(),
            }
        } else {
            ProseMode::SingleShot
        };
        run_live_h1_prose(
            vm,
            ctx,
            name,
            text,
            prose_mode,
            sys,
            reply,
            conversation,
            observer.as_ref(),
            debug.as_deref(),
            turns.as_ref(),
            client,
        )
        .await
    }
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
    sys: &serde_json::Value,
    title: &str,
) -> Result<()> {
    vm.inject_host(ctx.args(), sys, ctx.store(), None)?;
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
