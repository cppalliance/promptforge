//! The execute subtree's ambient run state.
//!
//! [`RunState`] is built once in [`run`](super::run) and travels through
//! the execute subtree as parameter one (`ctx: &RunState`). The
//! invariant: a new run-scoped concern becomes a field here, never a new
//! parameter. Per-call data (a section, a `var` snapshot) stays
//! in parameters or on the per-section frame.

use std::fmt;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

#[path = "context-bound.rs"]
mod bound;

use promptforge_api_types::emitter::{Emitter, EventSink};
use promptforge_api_types::event::Event;
use promptforge_api_types::ids::{ChainId, TaskId};

use crate::Result;
use crate::cancel::CancelHandle;
use crate::lua::{LuaProgram, ToolSet, ToolView};
use crate::model::{ModelSet, ModelView};
use crate::parser::Prompt;
use crate::store::{Access, VfsRef};
use crate::untrusted::GuardNonce;

use super::config::{RunContext, RunLimits};
use super::section_vm::{SectionVmSetup, VmSeed};
use super::support::sys_json;
use bound::{bound_model_set, bound_tool_set, derive_argv};

/// The ambient state one run shares across the execute subtree.
///
/// Immutable for the run's lifetime and cheap to clone: every field is
/// shared ownership or `Copy`, so a clone points at the same run state.
/// The three sanctioned forks: [`with_walk_state`](Self::with_walk_state)
/// at the H1-to-walk handoff, [`with_task`](Self::with_task) giving a
/// spawned chain its own task emitter and turn counter, and
/// [`with_args`](Self::with_args) carrying a `call` call's args override
/// into its contained chain.
#[derive(Clone)]
pub(crate) struct RunState {
    /// The prompt this run executes.
    prompt: Arc<Prompt>,
    /// The untrusted-envelope nonce, derived once here from the run's seed
    /// so every wrap in the run shares it.
    nonce: GuardNonce,
    /// The run's VFS handle: carries the store mount backing every
    /// section's Lua `store` table. Chain steps acquire or spawn their
    /// access capabilities from it.
    vfs: VfsRef,
    /// The execution identifier every observation carries.
    execution: Arc<str>,
    /// The run's argument string for `{{ args }}` substitution.
    args: Arc<str>,
    /// The run's `argv`: the parsed form of `args` (`None` installs nil).
    /// At construction this is the derived value the H1 pass starts from;
    /// the walk's fork carries the value H1 left behind at the freeze, so
    /// an H1 repair reaches every downstream section.
    argv: Option<Arc<serde_json::Value>>,
    /// The run's resource limits.
    limits: RunLimits,
    /// The run's event buffer, shared by every chain's emitter and every
    /// spawned leaf task, drained by the driver after each dispatch round.
    events: EventSink,
    /// This context's task-scoped emitter: the root task's at
    /// construction, a spawned chain's own after [`with_task`](Self::with_task).
    /// The Lua layer's seams (the shared replay, `log`, teardown, the
    /// shared tool-dispatch body) take it too, so their reports land in the
    /// buffer in order with the scheduler's own.
    emitter: Arc<Emitter>,
    /// Test-only: the host seams the suites set on their `RunContext`,
    /// carried here so the test driver's constructor can build its
    /// `RunHost` from the state alone. Shared, so a suite arms the tool
    /// implementations on a state it holds by reference.
    #[cfg(test)]
    test_host: Arc<Mutex<crate::test_support::RunHost>>,
    /// The run's cancel flag: polled between chain steps and installed on
    /// every section VM's instruction hook. The context's one handle, the
    /// same flag the activated capabilities and the run's `cancel` share.
    cancel: CancelHandle,
    /// Test-only: a copy of every drained event, so a test can assert on
    /// the values themselves - their provenance included - rather than on
    /// what the host observer was handed.
    #[cfg(test)]
    tap: Option<Arc<Mutex<Vec<Event>>>>,
    /// The model-turn counter this context advances (the run's, or one
    /// shared by all arms of a fanout).
    turns: Arc<AtomicU32>,
    /// The shared library replayed as every section's first chunk; an empty
    /// compiled chunk when the prompt declares no `lua shared` library, so
    /// the startup sequence carries no `Option` branch.
    shared: Arc<LuaProgram>,
    /// The run's tool set as a read-only view: built from the prepared
    /// bindings at construction. The trait exposes no write methods; the
    /// only writer is `tools.always` (a prompt-wide fact) through the
    /// concrete handle the section VMs share.
    tools: Arc<dyn ToolView>,
    /// The concrete handle behind `tools`, shared with every section VM
    /// (H1 included). Readers outside the VM layer go through the view.
    tool_set: Arc<Mutex<ToolSet>>,
    /// The run's model set as a read-only view: built from the prepared
    /// bindings at construction. The trait exposes no write methods; the
    /// only writer is `models.default` (a prompt-wide fact) through the
    /// concrete handle the section VMs share.
    models: Arc<dyn ModelView>,
    /// The concrete handle behind `models`, shared with every section VM
    /// (H1 included). Readers outside the VM layer go through the view.
    model_set: Arc<Mutex<ModelSet>>,
    /// The run's `started_at` rendered as RFC 3339, stamped into every
    /// section's `sys.when`, the H1 pass included.
    when: Arc<str>,
    /// The run's host-state snapshot; its presence is the Agent-window
    /// context (the `ui()` global plus raw-id `models.get`).
    ui: Option<Arc<serde_json::Value>>,
    /// Test-only: install the raw protocol shims (`models.chat`,
    /// `tools.call_as_model`) in every section VM, so a fixture section
    /// can yield one raw `chat` round or one model-issued `tool_call` at
    /// the scheduler's dispatch arms without going through a loop shim.
    #[cfg(test)]
    raw_shims: bool,
}

impl RunState {
    /// Builds the context for one run of `prompt`. The turn counter is
    /// minted here (starting at zero), as are the run's shared tool
    /// and model sets - built from the prepared bindings on `ctx` (empty on
    /// a caller-built context that never passed through
    /// [`Environment::prepare`](super::Environment::prepare), which runs
    /// capability-free); the nonce derives from `ctx`'s seed and `when`
    /// renders `ctx`'s `started_at`, so two contexts over the same inputs
    /// agree on both.
    #[must_use]
    pub(crate) fn new(
        prompt: Arc<Prompt>,
        args: &str,
        vfs: &VfsRef,
        shared: LuaProgram,
        ctx: &RunContext,
    ) -> Self {
        let tool_set = Arc::new(Mutex::new(bound_tool_set(&prompt, ctx)));
        let model_set = Arc::new(Mutex::new(bound_model_set(&prompt, ctx)));
        let execution: Arc<str> = Arc::from(ctx.name.as_str());
        let events = EventSink::default();
        // The root chain - the main walk - is task `0`.
        let emitter = Arc::new(Emitter::new(
            events.clone(),
            TaskId::from(ChainId::root()),
            Arc::clone(&execution),
            ctx.report_debug,
        ));
        let derived_argv = derive_argv(&prompt, args).map(Arc::from);
        Self {
            prompt,
            nonce: GuardNonce::from_seed(ctx.seed),
            vfs: vfs.clone(),
            execution,
            args: Arc::from(args),
            argv: derived_argv,
            limits: ctx.limits,
            events,
            emitter,
            #[cfg(test)]
            test_host: Arc::new(Mutex::new(ctx.test_host.clone())),
            cancel: ctx.cancel.clone(),
            #[cfg(test)]
            tap: None,
            turns: Arc::new(AtomicU32::new(0)),
            shared: Arc::new(shared),
            tools: tool_set.clone(),
            tool_set,
            models: model_set.clone(),
            model_set,
            when: Arc::from(ctx.started_at.to_rfc3339()),
            ui: ctx.ui.clone().map(Arc::new),
            #[cfg(test)]
            raw_shims: false,
        }
    }

    /// The host seams the suite set on its context, for the test driver's
    /// constructor.
    #[cfg(test)]
    pub(crate) fn test_host(&self) -> crate::test_support::RunHost {
        self.test_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replaces the test host: how a suite that builds the state itself
    /// arms the tool implementations or the observer its driver uses.
    #[cfg(test)]
    pub(crate) fn set_test_host(&self, host: crate::test_support::RunHost) {
        *self
            .test_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = host;
    }

    /// Exposes the raw protocol shims (`models.chat`, `tools.call_as_model`)
    /// in every section VM this run starts, so a test fixture can drive the
    /// scheduler's `Chat` arm with one raw round or its `tool_call` arm
    /// with one model-issued call.
    #[cfg(test)]
    pub(crate) fn expose_raw_shims_for_test(&mut self) {
        self.raw_shims = true;
    }

    /// Keeps a copy of every event the driver drains from this run's
    /// buffer, so a test can assert on the values - provenance included.
    /// Install before the scheduler is built: the drain reads the tap
    /// through the scheduler's root context.
    #[cfg(test)]
    pub(crate) fn record_events_for_test(&mut self) -> Arc<Mutex<Vec<Event>>> {
        let tap = Arc::new(Mutex::new(Vec::new()));
        self.tap = Some(Arc::clone(&tap));
        tap
    }

    /// The prompt this run executes.
    pub(crate) fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// The prompt's shared handle, for a caller that must hold the tree
    /// independently of this context's borrow.
    pub(crate) fn prompt_arc(&self) -> &Arc<Prompt> {
        &self.prompt
    }

    /// The run's cancel flag.
    pub(crate) fn cancel(&self) -> &CancelHandle {
        &self.cancel
    }

    /// The run's untrusted-envelope nonce.
    pub(crate) fn nonce(&self) -> &GuardNonce {
        &self.nonce
    }

    /// The run's VFS handle.
    pub(crate) fn vfs(&self) -> &VfsRef {
        &self.vfs
    }

    /// The execution identifier every observation carries.
    pub(crate) fn execution(&self) -> &str {
        &self.execution
    }

    /// The run's argument string.
    pub(crate) fn args(&self) -> &str {
        &self.args
    }

    /// The run's `argv`: the parsed form of the args string, or `None`
    /// (nil) when it did not parse. On the walk this is the value H1 left
    /// behind at the freeze.
    pub(crate) fn argv(&self) -> Option<&serde_json::Value> {
        self.argv.as_deref()
    }

    /// The run's resource limits.
    pub(crate) fn limits(&self) -> RunLimits {
        self.limits
    }

    /// This context's task-scoped emitter: where every report the
    /// scheduler makes on this chain goes.
    pub(crate) fn emitter(&self) -> &Arc<Emitter> {
        &self.emitter
    }

    /// Drains the run's event buffer: every event pushed since the last
    /// drain, in push order. The run's `step` calls this once per step and
    /// hands the batch to the host.
    pub(crate) fn take_events(&self) -> Vec<Event> {
        let events = self.events.take();
        #[cfg(test)]
        if let Some(tap) = &self.tap {
            tap.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(events.iter().cloned());
        }
        events
    }

    /// The model-turn counter this context advances.
    pub(crate) fn turns(&self) -> &Arc<AtomicU32> {
        &self.turns
    }

    /// The concrete handle behind the tools view, shared with every
    /// section VM the run constructs.
    pub(crate) fn tool_set(&self) -> Arc<Mutex<ToolSet>> {
        Arc::clone(&self.tool_set)
    }

    /// An owned snapshot of the run's tool set (bindings plus `always`),
    /// read through the view.
    ///
    /// # Errors
    /// Returns [`Error::Lua`](crate::Error::Lua) if the set's mutex is
    /// poisoned.
    pub(crate) fn tool_set_snapshot(&self) -> Result<ToolSet> {
        Ok(ToolSet::from_parts(
            self.tools.bindings()?,
            self.tools.always()?,
        ))
    }

    /// The run's model set, read-only.
    pub(crate) fn models(&self) -> &dyn ModelView {
        &*self.models
    }

    /// The concrete handle behind the models view, shared with every
    /// section VM the run constructs.
    pub(crate) fn model_set(&self) -> Arc<Mutex<ModelSet>> {
        Arc::clone(&self.model_set)
    }

    /// The resolved per-section tool-loop cap: the frontmatter's
    /// `max_tool_iterations` over the limits default.
    pub(crate) fn max_tool_iterations(&self) -> usize {
        self.prompt
            .frontmatter()
            .max_tool_iterations()
            .resolve(self.limits.tool_iterations().get() as usize)
    }

    /// The run's top-level section count, reported as `sys.section_count`.
    pub(crate) fn section_count(&self) -> usize {
        self.prompt.sections().len()
    }

    /// The H1-to-walk handoff: the `argv` H1 left behind at the freeze,
    /// set on a cheap clone so the context H1 saw stays untouched. The
    /// tool and model sets need no delta: they were built from the
    /// prepared bindings at construction, and H1's prompt-wide records
    /// (`tools.always`, `models.default`) landed in the same shared sets
    /// the views read. `when` needs none either: it is the run's
    /// `started_at`, the same for the pass and the walk.
    #[must_use]
    pub(crate) fn with_walk_state(&self, argv: Option<serde_json::Value>) -> Self {
        let mut ctx = self.clone();
        ctx.argv = argv.map(Arc::from);
        ctx
    }

    /// The context a contained chain runs under: `args` in place of the
    /// run's own, because a `call` call's explicit input overrides the
    /// run's args for the chain - and `argv` re-derives from the chain's
    /// args, so the chain sees the parsed form of what it was passed.
    #[must_use]
    pub(crate) fn with_args(&self, args: &str) -> Self {
        let mut ctx = self.clone();
        ctx.argv = derive_argv(&self.prompt, args).map(Arc::from);
        ctx.args = Arc::from(args);
        ctx
    }

    /// The context a spawned task chain runs under: an emitter stamping
    /// the chain's own `task` on every report, and `turns` in place of the
    /// run's counter, so the task's turns count against its own cap.
    #[must_use]
    pub(crate) fn with_task(&self, task: TaskId, turns: Arc<AtomicU32>) -> Self {
        let mut ctx = self.clone();
        ctx.emitter = Arc::new(self.emitter.for_task(task));
        ctx.turns = turns;
        ctx
    }

    /// The borrowed VM-setup inputs both engine drivers share, sourcing the
    /// run-wide slots (`args`, the emitter, `shared`, the shim caps) from
    /// this context; the driver supplies only its own deltas: the `sys`
    /// JSON, the seed, the chain step's access capability (the walk's own,
    /// a call chain's borrowed parent capability, a task chain's spawned
    /// one), and the section name.
    pub(crate) fn vm_setup<'a>(
        &'a self,
        sys: &'a serde_json::Value,
        seed: VmSeed<'a>,
        access: &'a Arc<Access>,
        section_name: &'a str,
    ) -> SectionVmSetup<'a> {
        SectionVmSetup {
            args: &self.args,
            argv: self.argv(),
            argv_writable: false,
            sys,
            access,
            seed,
            emitter: &self.emitter,
            section_name,
            shared: &self.shared,
            max_tool_iterations: self.max_tool_iterations(),
            max_fanout_concurrency: self.limits.fanout_concurrency().get(),
            ui: self.ui.as_ref(),
            #[cfg(test)]
            raw_shims: self.raw_shims,
        }
    }

    /// The `sys` JSON for one section or arm of this run under the run's
    /// `when`, with the driver supplying only the section entry's
    /// hierarchical id, the entering chain's task id, and the section name.
    pub(crate) fn sys_json(
        &self,
        id: &str,
        task_id: &TaskId,
        section_name: &str,
    ) -> serde_json::Value {
        sys_json(
            &self.when,
            id,
            &task_id.to_string(),
            section_name,
            &self.execution,
            self.section_count(),
        )
    }
}

impl fmt::Debug for RunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut state = f.debug_struct("RunState");
        #[cfg(test)]
        state
            .field("raw_shims", &self.raw_shims)
            .field("tap", &self.tap.is_some())
            .field("test_host", &self.test_host);
        state
            .field("prompt", &self.prompt)
            .field("nonce", &self.nonce)
            .field("vfs", &"<VfsRef>")
            .field("execution", &self.execution)
            .field("args", &self.args)
            .field("argv", &self.argv)
            .field("limits", &self.limits)
            .field("events", &self.events)
            .field("emitter", &self.emitter)
            .field("cancel", &self.cancel)
            .field("turns", &self.turns)
            .field("shared", &self.shared)
            .field("tools", &"<dyn ToolView>")
            .field("tool_set", &self.tool_set)
            .field("models", &"<dyn ModelView>")
            .field("model_set", &self.model_set)
            .field("when", &self.when)
            .field("ui", &self.ui)
            .finish()
    }
}

#[cfg(test)]
#[path = "context-tests.rs"]
mod tests;
