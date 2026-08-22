//! The per-section frame: one section entry's owned state within a run.
//!
//! [`SectionContext`] is born at a section entry and dies at its teardown.
//! It owns the section VM plus the state the block walk reads and writes -
//! the `sys` JSON, the seeded `var`, the rolling reply, the conversation,
//! the tool-call counts, and the resolved completion options - and it
//! carries the frame's effective reporting handles (observer, debug sink,
//! turn counter), so a fanout arm's proxies can arrive through the frame
//! rather than a forged context. Each driver is one
//! construct-run-teardown cycle: the constructor absorbs the VM
//! construction and setup preamble ([`SectionContext::new`] for a walked
//! section, [`SectionContext::new_live_h1`] for the live H1 pass,
//! [`SectionContext::new_fanout_arm`] for a fanout arm),
//! [`SectionContext::run`] is the ordered block walk, and
//! [`SectionContext::teardown`] is the single teardown boundary.
//!
//! All three drivers - the walk's `run_one_section`, the live H1 pass, and
//! the fanout arm - are one construct-run-teardown cycle over the frame,
//! differing only in seed and [`BlockRunMode`]. The run-scoped inputs
//! (bindings, models, limits, the shared registry) still arrive through the
//! borrowed [`RunFrame`]; they migrate to the `RunContext` in a later pass.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::Result;
use crate::client::{GatewayClient, Message};
use crate::debug::DebugCapture;
use crate::lua::{SectionVm, ToolCallCounts};
use crate::model::CompletionOptions;
use crate::observe::{Observer, detail};
use crate::parser::{Block, Section};
use crate::resolve::RuntimeResolution;
use crate::store::{StoreRef, WriteScope};

use super::block_walk::{BlockRunMode, SectionFlow, run_one_section_impl};
use super::context::RunContext;
use super::engine::{ControlContext, RunFrame, make_control_globals};
use super::gateway::GatewaySource;
use super::section_vm::{VmSeed, setup_section_vm};
use super::support::{next_id, now_rfc3339_checked, sys_json};
use super::tools::attach_infer_hook;

/// One section entry's owned frame within a run.
///
/// The frame is born at a section entry and dies at its teardown. One
/// section entry is one frame, regardless of arrival mode (fall-through,
/// jump, execute); a jump ends the current frame and the driver builds a
/// fresh one for the target - only `reply` and `var` cross, as call data.
/// No derives: the VM and the trait-object handles support neither `Clone`
/// nor `Debug`, matching [`ControlContext`].
pub(crate) struct SectionContext {
    /// The frame's engine: the owned section VM. `SectionVm` stays a
    /// standalone type in `lua/` with its own test suite - composition, not
    /// merger.
    vm: SectionVm,
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
    /// The store-write identity of a fanout arm; `None` on the walk, whose
    /// store writes stay untracked.
    write_scope: Option<WriteScope>,
    /// The section's `execute()` nesting depth, checked at the control
    /// globals the frame installs.
    execute_depth: usize,
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
    /// construction and limits, the control globals (resolved over the
    /// section's visible set), the shared setup half (host injection, host
    /// APIs, the shared replay, the captured alias bindings), and the infer
    /// hook.
    ///
    /// `siblings` is the caller's own walk slice, from which the section's
    /// visible set (its siblings minus itself, plus its direct children) is
    /// built for the control globals. `section_id` is the section's
    /// `sys.id`: the next value from the run-global counter.
    /// `incoming_reply` is the model reply visible to this section's first
    /// prose. `var` is the walk's current clipboard, seeded into the
    /// section's VM. `client` is the walk's client snapshot at this
    /// section's start: the control-global closures and the infer hook each
    /// capture a clone, so the persistent Lua closures hold no borrows.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction or limits failure propagates bare, before any teardown
    /// observation exists; a setup failure tears the fresh VM down first, so
    /// the teardown boundary still fires exactly once on that path.
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame's construction absorbs the walk's whole driver preamble: the shared frame and control context, the section and its home slice, its id, depth, reply seed, client snapshot, and var seed stay explicit and linear"
    )]
    #[expect(
        clippy::ref_option,
        reason = "the client snapshot is cloned into the control-global closures and the infer hook, so the parameter borrows the Option itself"
    )]
    pub(crate) fn new(
        frame: &RunFrame<'_>,
        control: &Arc<ControlContext>,
        section: &Section,
        siblings: &[Section],
        section_id: u64,
        execute_depth: usize,
        incoming_reply: Option<&str>,
        client: &Option<GatewayClient>,
        var: &serde_json::Value,
    ) -> Result<Self> {
        let sys = control.sys_json(section_id, &section.name)?;
        frame
            .observer
            .observe(frame.execution, &section.name, detail::SECTION_STARTED);
        let vm = SectionVm::new_for_section(
            frame.nonce,
            frame.bindings,
            frame.models,
            frame.execution,
            frame.observer.as_ref(),
            &section.name,
        )?;
        // A limits failure propagates bare: no teardown runs here, so no
        // LUA_TEARDOWN_* observation fires on this path.
        vm.apply_lua_limits(
            frame.limits.lua_memory().get(),
            frame.limits.lua_logs().get(),
        )?;
        let mut section_frame = Self {
            vm,
            sys,
            var: var.clone(),
            item: None,
            reply: incoming_reply.map(str::to_owned),
            conversation: Vec::new(),
            counts: None,
            completion_options: None,
            // Walk-section store writes are untracked; only fanout arms
            // carry a write scope.
            write_scope: None,
            execute_depth,
            observer: Arc::clone(frame.observer),
            debug: frame.debug.cloned(),
            turns: Arc::clone(frame.turns),
        };
        // The control globals are installed once for the section's whole
        // lifecycle; their callbacks capture the run-wide control context
        // plus the client snapshot, so they hold no borrows.
        let (execute_callback, fanout_callback, list_callback) = make_control_globals(
            control,
            client,
            section.clone(),
            siblings.to_vec(),
            section_frame.execute_depth,
            section_frame.reply.clone(),
        );
        // The setup half of the section lifecycle - host injection, host
        // APIs, the control globals, the shared replay, and the captured
        // alias bindings - is shared with the fanout arm; only the seed, the
        // `sys` extras, and the callbacks' parameters (home slice, caller,
        // depth) are the walk's own.
        let setup = control.vm_setup(
            &section_frame.sys,
            section_frame.reply.as_deref(),
            VmSeed {
                var: Some(&section_frame.var),
                item: section_frame.item.as_ref(),
            },
            section_frame.write_scope,
            &section.name,
        );
        if let Err(error) = setup_section_vm(
            &mut section_frame.vm,
            &setup,
            execute_callback,
            fanout_callback,
            list_callback,
        ) {
            section_frame.teardown(&section.name);
            return Err(error);
        }
        // The infer hook carries a lazy client source (F5): a nested
        // `models.infer` or `handle:infer` surfaces a concrete construction
        // error on first use instead of the setup swallowing it.
        control.attach_infer_hook(&section_frame.vm, client.clone(), &section.name);
        Ok(section_frame)
    }

    /// Constructs the frame for the live H1 pass and runs its setup
    /// preamble: the `sys` JSON (id 0 under the prompt's title), VM
    /// construction and limits, host injection, the host APIs, the H1
    /// control-global stubs, and the infer hook carrying the live binding
    /// producer, so a nested `models.infer` resolves against the bindings
    /// recorded so far.
    ///
    /// H1 is the level-1 section: it runs first and is never re-entered, so
    /// the frame seeds an empty `var`, no reply, no item, and no write
    /// scope. `client` is the run's client, cloned into the infer hook's
    /// lazy source (F5); the pass's own client slot stays with the caller.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction or limits failure propagates bare, before any teardown
    /// observation exists; a setup failure tears the fresh VM down first, so
    /// the teardown boundary still fires exactly once on that path.
    pub(crate) fn new_live_h1(
        ctx: &RunContext,
        frame: &RunFrame<'_>,
        runtime: &RuntimeResolution<'_, '_>,
        client: Option<&GatewayClient>,
    ) -> Result<Self> {
        let title = &ctx.prompt().title;
        let now = now_rfc3339_checked()?;
        let sys = sys_json(
            &now,
            &now,
            0,
            title,
            frame.execution,
            ctx.prompt().sections.len(),
        );
        let vm = SectionVm::new(ctx.nonce(), frame.execution, frame.observer.as_ref(), title)?;
        // A limits failure propagates bare: no teardown runs here, so no
        // LUA_TEARDOWN_* observation fires on this path.
        vm.apply_lua_limits(
            frame.limits.lua_memory().get(),
            frame.limits.lua_logs().get(),
        )?;
        let mut h1_frame = Self {
            vm,
            sys,
            var: serde_json::json!({}),
            item: None,
            reply: None,
            conversation: Vec::new(),
            counts: None,
            completion_options: None,
            write_scope: None,
            // H1 installs the control-global stubs, not the callbacks, so
            // no depth check ever reads this.
            execute_depth: 0,
            observer: Arc::clone(frame.observer),
            debug: frame.debug.cloned(),
            turns: Arc::clone(frame.turns),
        };
        if let Err(error) = h1_frame.setup_live_h1(frame.args, frame.store, title) {
            h1_frame.teardown(title);
            return Err(error);
        }
        // The infer hook carries a lazy client source (F5): a nested
        // `models.infer` or `handle:infer` surfaces a concrete construction
        // error on first use instead of the setup swallowing it.
        attach_infer_hook(
            &h1_frame.vm,
            GatewaySource::from_optional(client.cloned(), frame.limits),
            Arc::clone(&h1_frame.observer),
            h1_frame.debug.clone(),
            frame.execution,
            title,
            &h1_frame.turns,
            Some(runtime.producer()),
        );
        Ok(h1_frame)
    }

    /// The fallible setup half of the live H1 lifecycle: host injection,
    /// the host APIs, and the control-global stubs. One method, so the
    /// constructor's single teardown-on-error branch covers every step.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed.
    fn setup_live_h1(&mut self, args: &str, store: &StoreRef, title: &str) -> Result<()> {
        self.vm.inject_host(args, &self.sys, store, None)?;
        self.vm.install_host_apis(&self.observer, title)?;
        self.vm.install_h1_control_stubs()
    }

    /// Constructs the frame for one fanout arm and runs its setup preamble:
    /// VM construction and limits, the `sys` JSON carrying the arm's
    /// run-global `id` and its 1-based per-fanout `index`, the control
    /// globals (resolved over the worker's visible set: its home slice plus
    /// its children), the shared setup half, and the infer hook.
    ///
    /// The seed is the fanout's own: the collection `item`, the store-write
    /// scope (this fanout's token plus the arm's index, matching
    /// `sys.index`), the caller's cloned `var`, the caller's reply, and one
    /// execute level deeper than the caller. The effective reporting handles
    /// are the fanout's too: the proxy observer and debug sink (report-only
    /// traffic over the bounded side channels) and the fanout's fresh turn
    /// counter arrive as frame fields, not through a forged context. The
    /// shared control context carries the same handles, so the arm's nested
    /// `execute`/`fanout` chains report through the proxies as well.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed. A VM
    /// construction failure propagates bare - no VM exists to tear down. A
    /// limits, `sys`, or setup failure tears the fresh VM down once here:
    /// the arm's epilogue owns the run phase's teardown boundary, so the
    /// construction phase keeps its own and every path tears down exactly
    /// once.
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame's construction absorbs the arm's whole driver preamble: the shared control context, the worker and its home slice, the arm's position, item, write token, depth, reply and var seeds, client snapshot, and effective reporting handles stay explicit and linear"
    )]
    #[expect(
        clippy::ref_option,
        reason = "the client snapshot is cloned into the control-global closures and the infer hook, so the parameter borrows the Option itself"
    )]
    pub(crate) fn new_fanout_arm(
        control: &Arc<ControlContext>,
        worker: &Section,
        home: &[Section],
        index: usize,
        item: serde_json::Value,
        write_token: u64,
        execute_depth: usize,
        incoming_reply: Option<&str>,
        client: &Option<GatewayClient>,
        var: &serde_json::Value,
        observer: Arc<dyn Observer>,
        debug: Option<Arc<dyn DebugCapture>>,
        turns: Arc<AtomicU32>,
    ) -> Result<Self> {
        let vm = SectionVm::new_for_section(
            &control.nonce,
            &control.bindings,
            &control.models,
            &control.execution,
            observer.as_ref(),
            &worker.name,
        )?;
        // The limits install and the `sys` build are the construction
        // phase's fallible steps once the VM exists; a failure tears the
        // fresh VM down once here, matching the single teardown the arm's
        // epilogue owns for the run phase.
        let sys = match vm
            .apply_lua_limits(
                control.limits.lua_memory().get(),
                control.limits.lua_logs().get(),
            )
            .and_then(|()| {
                let mut sys = control.sys_json(next_id(&control.ids), &worker.name)?;
                // The arm's own sys extra: its 1-based position within this
                // fanout. Absent outside a fanout, so a walked section
                // reading `sys.index` raises the sealed-sys unknown-field
                // error; a nested fanout's arms restart at 1.
                sys["index"] = serde_json::Value::from(index + 1);
                Ok(sys)
            }) {
            Ok(sys) => sys,
            Err(error) => {
                vm.teardown(observer.as_ref(), &worker.name);
                return Err(error);
            }
        };
        let mut arm_frame = Self {
            vm,
            sys,
            var: var.clone(),
            item: Some(item),
            reply: incoming_reply.map(str::to_owned),
            conversation: Vec::new(),
            counts: None,
            completion_options: None,
            // The arm's store-write identity: this fanout's token plus the
            // arm's 1-based index, matching `sys.index`.
            write_scope: Some(WriteScope::new(write_token, index + 1)),
            execute_depth,
            observer,
            debug,
            turns,
        };
        // The control globals are installed once for the arm's whole
        // lifecycle; their callbacks capture the run-wide control context
        // plus the client snapshot, so they hold no borrows.
        let (execute_callback, fanout_callback, list_callback) = make_control_globals(
            control,
            client,
            worker.clone(),
            home.to_vec(),
            arm_frame.execute_depth,
            arm_frame.reply.clone(),
        );
        // The setup half is shared with the walk; only the seed, the `sys`
        // extra, and the callbacks' parameters (home slice, caller, depth)
        // are the arm's own.
        let setup = control.vm_setup(
            &arm_frame.sys,
            arm_frame.reply.as_deref(),
            VmSeed {
                var: Some(&arm_frame.var),
                item: arm_frame.item.as_ref(),
            },
            arm_frame.write_scope,
            &worker.name,
        );
        if let Err(error) = setup_section_vm(
            &mut arm_frame.vm,
            &setup,
            execute_callback,
            fanout_callback,
            list_callback,
        ) {
            arm_frame.teardown(&worker.name);
            return Err(error);
        }
        // The infer hook carries a lazy client source (F5): a nested
        // `models.infer` or `handle:infer` surfaces a concrete construction
        // error on first use instead of the setup swallowing it.
        control.attach_infer_hook(&arm_frame.vm, client.clone(), &worker.name);
        Ok(arm_frame)
    }

    /// Runs the frame's ordered block walk: Lua chunks in place, prose
    /// through the tool loop, the reply rolling forward.
    ///
    /// `frame` supplies the run-scoped inputs (the shared registry, the
    /// bindings, models, limits, and the tool-loop cap); everything
    /// per-frame - the VM, the `sys` JSON, the reply, the conversation, the
    /// counts, the completion options, and the effective reporting handles -
    /// comes from `self`. The caller owns the teardown boundary: a walk
    /// error propagates without tearing the VM down, so the driver's single
    /// teardown covers every path.
    ///
    /// # Errors
    /// Returns the [`Error`](crate::Error) of whichever step failed, as
    /// documented on [`run_one_section_impl`].
    pub(crate) async fn run(
        &mut self,
        frame: &RunFrame<'_>,
        name: &str,
        blocks: &[Block],
        mode: BlockRunMode<'_>,
        client: &mut Option<GatewayClient>,
    ) -> Result<SectionFlow> {
        run_one_section_impl(
            &mut self.vm,
            frame,
            name,
            blocks,
            mode,
            &mut self.sys,
            &mut self.reply,
            &mut self.conversation,
            &mut self.counts,
            &mut self.completion_options,
            self.item.as_ref(),
            self.observer.as_ref(),
            self.debug.as_deref(),
            self.turns.as_ref(),
            client,
        )
        .await
    }

    /// Reads the section's final `var` back into the frame and returns it,
    /// so the walk rolls it forward. Must run before
    /// [`teardown`](Self::teardown): the read goes through the live VM.
    ///
    /// # Errors
    /// Returns [`Error::Lua`](crate::Error::Lua) if the VM's `var` cannot be
    /// converted back to JSON (the write guard keeps this conversion from
    /// failing in practice).
    pub(crate) fn read_var(&mut self) -> Result<serde_json::Value> {
        self.var = self.vm.var()?;
        Ok(self.var.clone())
    }

    /// Tears the frame's VM down, reporting the teardown through the frame's
    /// observer.
    ///
    /// Consumes the frame: one section entry tears its VM down exactly once,
    /// at the driver's teardown boundary. This stays a method, never `Drop` -
    /// a fanout arm's VM must outlive its cancel-scoped body so the epilogue
    /// can finalize first.
    pub(crate) fn teardown(self, name: &str) {
        self.vm.teardown(self.observer.as_ref(), name);
    }
}
