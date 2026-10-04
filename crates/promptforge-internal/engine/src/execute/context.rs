//! The execute subtree's ambient run state.
//!
//! [`RunState`] is built once in [`run`](super::run) and travels through
//! the execute subtree as parameter one (`ctx: &RunState`). The
//! invariant: a new run-scoped concern becomes a field here, never a new
//! parameter. Per-call data (a section, a `var` snapshot) stays
//! in parameters or on the per-section frame.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex};

#[path = "context-bound.rs"]
mod bound;

use promptforge_types::capabilities::Prelude;
use promptforge_types::emitter::{Emitter, EventSink};
use promptforge_types::event::Event;
use promptforge_types::ids::{ChainId, TaskId};

use crate::Result;
use crate::cancel::CancelHandle;
use crate::lua::{LuaProgram, ToolBinding, ToolSet, ToolView};
use crate::model::{ModelSet, ModelView};
use crate::parser::Prompt;
use crate::untrusted::GuardNonce;
use promptforge_vfs::{Access, VfsRef};

use super::config::{RunContext, RunLimits};
use super::section_vm::{SectionVmSetup, VmSeed};
use super::support::sys_json;
use bound::{bound_model_set, bound_tool_set, catalog_bindings, derive_argv, frontmatter_aliases};

/// The ambient state one run shares across the execute subtree.
///
/// Immutable for the run's lifetime and cheap to clone: every field is
/// shared ownership or `Copy`, so a clone points at the same run state.
/// The three sanctioned forks: [`with_walk_state`](Self::with_walk_state)
/// at the H1-to-walk handoff, [`with_task`](Self::with_task) giving a
/// spawned chain its own task emitter and turn counter, and
/// [`with_args`](Self::with_args) passing a `call` call's args override
/// into its contained chain.
#[derive(Clone)]
pub(crate) struct RunState {
    /// The prompt this run executes.
    prompt: Arc<Prompt>,
    /// The untrusted-envelope nonce, derived once here from the run's seed
    /// so every wrap in the run shares it.
    nonce: GuardNonce,
    /// The run's VFS handle: the run's whole filesystem, the declared
    /// store backing every section's Lua `store` table included. Chain
    /// steps acquire or spawn their access capabilities from it.
    vfs: VfsRef,
    /// The execution identifier stamped on every observation.
    execution: Arc<str>,
    /// The run's argument string for `{{ args }}` substitution.
    args: Arc<str>,
    /// The run's `argv`: the parsed form of `args` (`None` installs nil).
    /// At construction this is the derived value the H1 pass starts from;
    /// the walk's fork holds the value H1 left behind at the freeze, so
    /// an H1 repair reaches every downstream section.
    argv: Option<Arc<serde_json::Value>>,
    /// The run's resource limits.
    limits: RunLimits,
    /// The run's event buffer, shared by every chain's emitter, spawned
    /// task chains' included, and drained once per `step` into the batch
    /// handed to the Harness.
    events: EventSink,
    /// This context's task-scoped emitter: the root task's at
    /// construction, a spawned chain's own after [`with_task`](Self::with_task).
    /// The Lua layer's seams (the shared replay, `log`, teardown, the
    /// shared tool-dispatch body) take it too, so their reports land in the
    /// buffer in order with the scheduler's own.
    emitter: Arc<Emitter>,
    /// The run's cancel flag: polled between chain steps and installed on
    /// every section VM's instruction hook. The context's one handle, the
    /// same flag the activated capabilities and the run's `cancel` share.
    cancel: CancelHandle,
    /// Test-only: a copy of every drained event, so a test can assert on
    /// the values themselves - their provenance included - without
    /// collecting each step's batch.
    #[cfg(test)]
    tap: Option<Arc<Mutex<Vec<Event>>>>,
    /// The model-turn counter this context advances: the run's, or a
    /// spawned task chain's own from [`with_task`](Self::with_task).
    turns: Arc<AtomicU32>,
    /// The shared library replayed as every section's first chunk; an empty
    /// compiled chunk when the prompt declares no `lua shared` library, so
    /// the startup sequence needs no `Option` branch.
    shared: Arc<LuaProgram>,
    /// The run's tool set as a read-only view: built from the prepared
    /// bindings at construction. The only writer is `tools.always` (a
    /// prompt-wide fact) through the concrete handle the section VMs
    /// share.
    tools: Arc<dyn ToolView>,
    /// The concrete handle behind `tools`, shared with every section VM
    /// (H1 included). Readers outside the VM layer go through the view.
    tool_set: Arc<Mutex<ToolSet>>,
    /// Every tool in the prepared catalog, bound under its full id: the
    /// fallback a script `tools.call` resolves when no frontmatter alias
    /// matches. Kept apart from `tool_set`, the set section VMs install
    /// globals and scopes from, so a full id never becomes either.
    catalog_bindings: Arc<BTreeMap<String, ToolBinding>>,
    /// The run's model set as a read-only view: built from the prepared
    /// bindings at construction. The only writer is `models.default` (a
    /// prompt-wide fact) through the concrete handle the section VMs
    /// share.
    models: Arc<dyn ModelView>,
    /// The concrete handle behind `models`, shared with every section VM
    /// (H1 included). Readers outside the VM layer go through the view.
    model_set: Arc<Mutex<ModelSet>>,
    /// The run's `started_at` rendered as RFC 3339, stamped into every
    /// section's `sys.when`, the H1 pass included.
    when: Arc<str>,
    /// The run's Host-state snapshot; its presence gives every section VM
    /// the `ui()` global and the raw-model-id `models.get` fallback.
    ui: Option<Arc<serde_json::Value>>,
    /// The run's capability preludes, in install order: every section VM
    /// installs each one before the shared library replays.
    preludes: Arc<[Prelude]>,
    /// Every tool and model alias the prompt's frontmatter declares: the
    /// names a prelude's globals must not take, because the alias globals
    /// install after the preludes and would silently replace them.
    frontmatter_aliases: Arc<[String]>,
    /// Test-only: installs the raw `tools.call_as_model` shim in every
    /// section VM, so a fixture section can yield one model-issued
    /// `tool_call` at the scheduler's dispatch arm without going through a
    /// loop shim.
    #[cfg(test)]
    raw_shims: bool,
}

impl RunState {
    /// Builds the context for one run of `prompt`. The turn counter is
    /// minted here (starting at zero), as are the run's shared tool
    /// and model sets - built from the prepared bindings on `ctx` (empty on
    /// a caller-built context that never passed through
    /// [`Environment::prepare`](super::Environment::prepare), which runs
    /// capability-free) - the full-id bindings of `ctx`'s catalog, and the
    /// prompt's frontmatter alias names that `ctx`'s preludes are checked
    /// against; the nonce derives from `ctx`'s seed and `when`
    /// renders `ctx`'s `started_at`, so two contexts over the same inputs
    /// agree on both.
    #[must_use]
    pub(super) fn new(
        prompt: Arc<Prompt>,
        args: &str,
        vfs: &VfsRef,
        shared: LuaProgram,
        ctx: &RunContext,
    ) -> Self {
        let tool_set = Arc::new(Mutex::new(bound_tool_set(&prompt, ctx)));
        let model_set = Arc::new(Mutex::new(bound_model_set(&prompt, ctx)));
        let execution: Arc<str> = Arc::from(ctx.name.as_str());
        // The root task's counter starts where the Harness says: past the
        // parse events it logged ahead of the run, or at zero.
        let events = EventSink::seeded(ctx.provenance_start);
        // The root chain - the main walk - is task `0`.
        let emitter = Arc::new(Emitter::new(
            events.clone(),
            TaskId::from(ChainId::root()),
            Arc::clone(&execution),
            ctx.report_debug,
        ));
        let derived_argv = derive_argv(&prompt, args).map(Arc::from);
        let frontmatter_aliases = frontmatter_aliases(&prompt).into();
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
            cancel: ctx.cancel.clone(),
            #[cfg(test)]
            tap: None,
            turns: Arc::new(AtomicU32::new(0)),
            shared: Arc::new(shared),
            tools: tool_set.clone(),
            tool_set,
            catalog_bindings: Arc::new(catalog_bindings(ctx)),
            models: model_set.clone(),
            model_set,
            when: Arc::from(ctx.started_at.to_rfc3339()),
            ui: ctx.ui.clone().map(Arc::new),
            preludes: ctx.preludes.as_slice().into(),
            frontmatter_aliases,
            #[cfg(test)]
            raw_shims: false,
        }
    }

    /// Exposes the raw `tools.call_as_model` shim in every section VM this
    /// run starts, so a test fixture can drive the scheduler's `tool_call`
    /// arm with one model-issued call.
    #[cfg(test)]
    pub(super) fn expose_raw_shims_for_test(&mut self) {
        self.raw_shims = true;
    }

    /// Keeps a copy of every event the driver drains from this run's
    /// buffer, so a test can assert on the values - provenance included.
    /// Install before the scheduler is built: the drain reads the tap
    /// through the scheduler's root context.
    #[cfg(test)]
    pub(super) fn record_events_for_test(&mut self) -> Arc<Mutex<Vec<Event>>> {
        let tap = Arc::new(Mutex::new(Vec::new()));
        self.tap = Some(Arc::clone(&tap));
        tap
    }

    /// The prompt this run executes.
    pub(super) fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    /// The prompt's shared handle, for a caller that must hold the tree
    /// independently of this context's borrow.
    pub(super) fn prompt_arc(&self) -> &Arc<Prompt> {
        &self.prompt
    }

    /// The run's cancel flag.
    pub(super) fn cancel(&self) -> &CancelHandle {
        &self.cancel
    }

    /// The run's untrusted-envelope nonce.
    pub(super) fn nonce(&self) -> &GuardNonce {
        &self.nonce
    }

    /// The run's VFS handle.
    pub(super) fn vfs(&self) -> &VfsRef {
        &self.vfs
    }

    /// The execution identifier stamped on every observation.
    pub(super) fn execution(&self) -> &str {
        &self.execution
    }

    /// The run's argument string.
    pub(super) fn args(&self) -> &str {
        &self.args
    }

    /// The run's `argv`: the parsed form of the args string, or `None`
    /// (nil) when it did not parse. On the walk this is the value H1 left
    /// behind at the freeze.
    pub(super) fn argv(&self) -> Option<&serde_json::Value> {
        self.argv.as_deref()
    }

    /// The run's resource limits.
    pub(crate) fn limits(&self) -> RunLimits {
        self.limits
    }

    /// This context's task-scoped emitter: where every report the
    /// scheduler makes on this chain goes.
    pub(super) fn emitter(&self) -> &Arc<Emitter> {
        &self.emitter
    }

    /// Drains the run's event buffer: every event pushed since the last
    /// drain, in push order. The run's `step` calls this once per step and
    /// hands the batch to the Harness.
    pub(super) fn take_events(&self) -> Vec<Event> {
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
    pub(super) fn turns(&self) -> &Arc<AtomicU32> {
        &self.turns
    }

    /// The concrete handle behind the tools view, shared with every
    /// section VM the run constructs.
    pub(super) fn tool_set(&self) -> Arc<Mutex<ToolSet>> {
        Arc::clone(&self.tool_set)
    }

    /// An owned snapshot of the run's tool set (bindings plus `always`),
    /// read through the view.
    ///
    /// # Errors
    /// Returns [`Error::Lua`](crate::Error::Lua) if the set's mutex is
    /// poisoned.
    pub(super) fn tool_set_snapshot(&self) -> Result<ToolSet> {
        Ok(ToolSet::from_parts(
            self.tools.bindings()?,
            self.tools.always()?,
        ))
    }

    /// The binding for the catalog tool whose full id is `id`, the
    /// fallback a script `tools.call` resolves when no frontmatter alias
    /// matches.
    pub(super) fn catalog_binding(&self, id: &str) -> Option<&ToolBinding> {
        self.catalog_bindings.get(id)
    }

    /// The run's model set, read-only.
    pub(super) fn models(&self) -> &dyn ModelView {
        &*self.models
    }

    /// The concrete handle behind the models view, shared with every
    /// section VM the run constructs.
    pub(super) fn model_set(&self) -> Arc<Mutex<ModelSet>> {
        Arc::clone(&self.model_set)
    }

    /// The resolved per-section tool-loop cap: the frontmatter's
    /// `max_tool_iterations` over the limits default.
    fn max_tool_iterations(&self) -> usize {
        promptforge_parser::detail::max_tool_iterations(self.prompt.frontmatter())
            .resolve(self.limits.tool_iterations().get() as usize)
    }

    /// The run's top-level section count, reported as `sys.section_count`.
    fn section_count(&self) -> usize {
        promptforge_parser::detail::sections(&self.prompt).len()
    }

    /// The H1-to-walk handoff: the `argv` H1 left behind at the freeze,
    /// set on a cheap clone so the context H1 saw stays untouched. The
    /// tool and model sets need no delta: they were built from the
    /// prepared bindings at construction, and H1's prompt-wide records
    /// (`tools.always`, `models.default`) landed in the same shared sets
    /// the views read. `when` needs none either: it is the run's
    /// `started_at`, the same for the pass and the walk.
    #[must_use]
    pub(super) fn with_walk_state(&self, argv: Option<serde_json::Value>) -> Self {
        let mut ctx = self.clone();
        ctx.argv = argv.map(Arc::from);
        ctx
    }

    /// The context a contained chain runs under: `args` in place of the
    /// run's own, because a `call` call's explicit input overrides the
    /// run's args for the chain - and `argv` re-derives from the chain's
    /// args, so the chain sees the parsed form of what it was passed.
    #[must_use]
    pub(super) fn with_args(&self, args: &str) -> Self {
        let mut ctx = self.clone();
        ctx.argv = derive_argv(&self.prompt, args).map(Arc::from);
        ctx.args = Arc::from(args);
        ctx
    }

    /// The context a spawned task chain runs under: an emitter stamping
    /// the chain's own `task` on every report, and `turns` in place of the
    /// run's counter, so the task's turns count against its own cap.
    #[must_use]
    pub(super) fn with_task(&self, task: TaskId, turns: Arc<AtomicU32>) -> Self {
        let mut ctx = self.clone();
        ctx.emitter = Arc::new(self.emitter.for_task(task));
        ctx.turns = turns;
        ctx
    }

    /// The borrowed VM-setup inputs both section drivers share, sourcing the
    /// run-wide slots (`args`, the emitter, `shared`, the shim caps, the
    /// preludes and the alias names they are checked against) from
    /// this context; the driver supplies only its own deltas: the `sys`
    /// JSON, the seed, the chain step's access capability (the walk's own,
    /// a call chain's borrowed parent capability, a task chain's spawned
    /// one), and the section name.
    pub(super) fn vm_setup<'a>(
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
            ui: self.ui.as_ref(),
            preludes: &self.preludes,
            frontmatter_aliases: &self.frontmatter_aliases,
            #[cfg(test)]
            raw_shims: self.raw_shims,
        }
    }

    /// The `sys` JSON for one section or arm of this run under the run's
    /// `when`, with the driver supplying only the section entry's
    /// hierarchical id, the entering chain's task id, and the section name.
    pub(super) fn sys_json(
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
            .field("tap", &self.tap.is_some());
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
            .field(
                "catalog_bindings",
                &self.catalog_bindings.keys().collect::<Vec<_>>(),
            )
            .field("models", &"<dyn ModelView>")
            .field("model_set", &self.model_set)
            .field("when", &self.when)
            .field("ui", &self.ui)
            .field(
                "preludes",
                &self
                    .preludes
                    .iter()
                    .map(Prelude::capability)
                    .collect::<Vec<_>>(),
            )
            .field("frontmatter_aliases", &self.frontmatter_aliases)
            .finish()
    }
}

#[cfg(test)]
#[path = "context-tests.rs"]
mod tests;
