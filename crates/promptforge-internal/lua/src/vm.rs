//! The per-section Lua VM: construction, Engine injection, coroutine stepping, and chunk execution.
//!
//! A local tool registered with `tools.offer_local` runs inside the calling
//! block's coroutine, never as an Engine function. The VM keeps only each
//! tool's alias and schema; the handler sits in the VM's handler table. A
//! call takes two yields: the shim's `tool_call`, which the scheduler
//! answers with the handler itself, and the shim's `local_tool_done`
//! after the handler ran, which carries its result for the scheduler to
//! report. Every suspending call the handler makes in between is an
//! ordinary yield of the same coroutine.

mod install;
mod run;
mod state;

use super::{
    Access, Arc, AtomicU32, AtomicUsize, BTreeMap, DEFAULT_LUA_LOG_EVENTS,
    DEFAULT_LUA_MEMORY_BYTES, Emitter, Error, GuardNonce, InstructionBudget, Json, Lua, LuaOptions,
    ModelRuntime, ModelSet, Mutex, Ordering, Result, StdLib, ToolRuntime, ToolSet, harden,
    install_deterministic_iteration, install_instruction_budget, install_untrusted, lifecycle,
    log_byte_budget,
};
use promptforge_model_client::client::ToolSchema;

pub use run::CoroStep;
#[cfg(test)]
pub(crate) use run::{LuaOutcome, run_chunk};
pub use state::{current_tool_bindings, resolve_model_binding};

/// Packs owned values into a 1-based Lua sequence table.
fn pack_sequence<T: mlua::IntoLua>(lua: &Lua, values: Vec<T>) -> mlua::Result<mlua::Table> {
    let table = lua.create_table_with_capacity(values.len(), 0)?;
    for (index, value) in values.into_iter().enumerate() {
        table.raw_set(index + 1, value)?;
    }
    Ok(table)
}

/// One hardened, isolated Lua VM for a section's complete lifecycle.
///
/// The VM owns one Lua environment from construction until drop. Construction
/// hardens the sandbox and installs `untrusted`; the caller then drives one
/// linear startup: apply the run's limits, inject the Engine values, install
/// the persistent Engine globals and the control globals, replay the shared
/// library as the section's first chunk
/// ([`replay_shared`](Self::replay_shared)), and only then walk the
/// section's blocks with
/// [`start_block_coro`](Self::start_block_coro). A single
/// instruction hook covers every program run by this VM, on the main state
/// and on every block coroutine, so cancellation reaches any chunk.
///
/// `SectionVm` deliberately does not expose its underlying [`Lua`]. This keeps
/// hardening, Engine injection, instruction accounting, and report delivery on
/// the one owned path. Each section must receive a new instance; dropping it
/// destroys all Lua memory belonging to that section. Once Lua allocation
/// succeeds, construction and shared-load failures cross the same explicit
/// observed teardown boundary as later lifecycle failures.
#[derive(Debug)]
pub struct SectionVm {
    lua: Lua,
    /// The run's shared tool set: the declared plugins, the offering, and
    /// the prompt-wide offers. Shared with the run, not snapshotted:
    /// `tools.always_offer` is a prompt-wide fact that later sections must
    /// see.
    bound_tools: Arc<Mutex<ToolSet>>,
    /// The run's shared model set: the frontmatter's filled roles plus the
    /// prompt-wide default. Shared for the same reason (`models.default`).
    bound_models: Arc<Mutex<ModelSet>>,
    /// The section's tool-addition runtime, read by the executor's scope path.
    pub tool_runtime: Arc<Mutex<ToolRuntime>>,
    /// The section's model-selection runtime, read by the executor's scope path.
    pub model_runtime: Arc<Mutex<ModelRuntime>>,
    /// Set by Lua `jump` before it aborts the current chunk.
    jump_slot: Arc<Mutex<Option<String>>>,
    /// How many local tool handlers are running on this VM; `jump` refuses
    /// while it is above zero.
    local_handler_depth: Arc<AtomicU32>,
    /// Live sealed `sys` JSON, mirrored for [`current_sys`](Self::current_sys)
    /// snapshots.
    sys_live: Arc<Mutex<Option<Json>>>,
    /// The section's VFS access capability: the `store` table's closures
    /// share it, so every store op is attributed to the identity the
    /// executor installed for this chain step.
    access: Option<Arc<Access>>,
    /// The slot the direct store closures record a claims-model conflict
    /// into during the shared replay, for the executor to read when the
    /// load returns via
    /// [`take_store_conflict`](Self::take_store_conflict).
    store_conflicts: Arc<Mutex<Option<String>>>,
    values_injected: bool,
    /// Remaining `log()` events this VM may emit before the budget is exhausted.
    log_budget: Arc<AtomicU32>,
    /// Remaining cumulative `log()` message bytes this VM may emit. Bounds total
    /// log volume even when each event is under the per-event ceilings.
    log_byte_budget: Arc<AtomicUsize>,
    /// Local tools registered by Lua code: alias and schema, for
    /// membership and advertising.
    local_tools: LocalTools,
    /// The VM's instruction-budget counter, shared with every block
    /// coroutine's hook (hooks are per-coroutine in PUC Lua).
    instruction_budget: InstructionBudget,
    /// The raw-model-id fallback, on whenever the caller passes an
    /// application-state snapshot (`RunContext::ui`): when set, `models.get`
    /// resolves an undeclared alias as a raw gateway catalog model id. Set by
    /// [`allow_raw_model_ids`](Self::allow_raw_model_ids) before Engine
    /// injection; unset everywhere else.
    raw_model_ids: bool,
    /// Test-support only: the live-section-VM tally guard. Its presence
    /// counts this VM as live from construction until the VM drops (its
    /// teardown); the Engine's scheduler suite reads the tally to pin the
    /// admission ceiling's cost - a queued task holds no VM.
    #[cfg(feature = "test-support")]
    _vm_tally: vm_tally::Guard,
}

/// Test-support only: a per-thread tally of live section VMs.
///
/// The Engine's scheduler suite drives one run on the test's own thread
/// (the tokio driver is `current_thread`), and Rust's built-in test harness gives
/// each test its own thread, so thread-locals keep concurrent tests
/// independent. A test resets the peak, drives a run, and reads back the
/// most VMs alive at any moment.
#[cfg(feature = "test-support")]
mod vm_tally {
    use std::cell::Cell;

    thread_local! {
        static LIVE: Cell<usize> = const { Cell::new(0) };
        static PEAK: Cell<usize> = const { Cell::new(0) };
    }

    /// One section VM holds one guard: entering counts it, dropping it
    /// releases the count.
    #[derive(Debug)]
    pub(super) struct Guard;

    impl Guard {
        pub(super) fn enter() -> Self {
            LIVE.with(|live| {
                let now = live.get() + 1;
                live.set(now);
                PEAK.with(|peak| peak.set(peak.get().max(now)));
            });
            Self
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            LIVE.with(|live| live.set(live.get() - 1));
        }
    }

    /// Resets the peak, so a test measures one run only.
    pub(super) fn reset_peak() {
        PEAK.with(|peak| peak.set(0));
    }

    /// The most section VMs alive at once since the last reset.
    pub(super) fn peak() -> usize {
        PEAK.with(Cell::get)
    }
}

/// Test-support only: resets the live-section-VM peak tally, so a test
/// measures the one run it drives next.
#[cfg(feature = "test-support")]
pub fn reset_section_vm_peak() {
    vm_tally::reset_peak();
}

/// Test-support only: the most section VMs alive at once since the last
/// [`reset_section_vm_peak`]: the admission test's cost pin.
#[cfg(feature = "test-support")]
#[must_use]
pub fn section_vm_peak() -> usize {
    vm_tally::peak()
}

/// Local tool registrations owned by a section VM.
///
/// Each entry holds the tool alias and its prebuilt schema, which serve
/// membership and advertising. The handler function itself lives in the
/// VM's handler table, written by `tools.offer_local` and read back when the
/// scheduler answers a call with it. The entries are shared with the
/// `tools.offer_local` Lua callback, which must be `Send`, hence the `Mutex`;
/// the VM is single-threaded, so the lock never contends.
#[derive(Debug, Default, Clone)]
pub(crate) struct LocalTools {
    entries: Arc<Mutex<Vec<(String, ToolSchema)>>>,
}

impl LocalTools {
    /// Registers a local tool: alias and prebuilt schema.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the entries lock was poisoned.
    pub(crate) fn register(&self, alias: String, schema: ToolSchema) -> Result<()> {
        self.entries
            .lock()
            .map_err(|_| Error::Lua("local tools registry was poisoned".to_owned()))?
            .push((alias, schema));
        Ok(())
    }

    /// Returns the schemas of every registered local tool.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the entries lock was poisoned.
    pub(crate) fn schemas(&self) -> Result<Vec<ToolSchema>> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| Error::Lua("local tools registry was poisoned".to_owned()))?
            .iter()
            .map(|(_, schema)| schema.clone())
            .collect())
    }

    /// Returns whether `alias` names a registered local tool.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the entries lock was poisoned.
    pub(crate) fn contains(&self, alias: &str) -> Result<bool> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| Error::Lua("local tools registry was poisoned".to_owned()))?
            .iter()
            .any(|(name, _)| name == alias))
    }

    #[cfg(test)]
    pub(crate) fn entries_handle(&self) -> Arc<Mutex<Vec<(String, ToolSchema)>>> {
        Arc::clone(&self.entries)
    }
}

impl SectionVm {
    /// Creates a hardened section VM.
    ///
    /// Construction installs only the sandbox (the `_G` guard included),
    /// the deterministic `pairs`/`next` walk, the default resource
    /// ceilings, the instruction hook, and `untrusted` (wrapping under the
    /// run's `nonce`). Everything else - the run's
    /// limits, the Engine values, the persistent Engine globals, the control
    /// globals, and the shared-library replay - is a separate explicit step
    /// the caller drives in that order (see the type-level docs). Every
    /// lifecycle report goes through the emitter the caller hands each
    /// step; the VM retains none.
    ///
    /// The VM shares an empty tool and model set, so the validating
    /// `tools.offer` installed by
    /// [`inject_values_with_var`](Self::inject_values_with_var) refuses
    /// every id.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM cannot be built or hardened.
    pub fn new(nonce: &GuardNonce, emitter: &Emitter, section: &str) -> Result<Self> {
        #[cfg(feature = "test-support")]
        let tally = vm_tally::Guard::enter();
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        )
        .map_err(Error::lua)?;
        // Bound the VM heap by default; `apply_lua_limits` may tighten or relax
        // it to the caller's `RunLimits`. A safe non-env default keeps every VM
        // bounded even when the run installs no explicit limits.
        lua.set_memory_limit(DEFAULT_LUA_MEMORY_BYTES)
            .map_err(Error::lua)?;
        let mut vm = Self {
            lua,
            bound_tools: Arc::new(Mutex::new(ToolSet::default())),
            bound_models: Arc::new(Mutex::new(ModelSet::default())),
            tool_runtime: Arc::new(Mutex::new(ToolRuntime {
                added: Vec::new(),
                description_overrides: BTreeMap::new(),
                allowed_tasks: None,
            })),
            model_runtime: Arc::new(Mutex::new(ModelRuntime::new())),
            jump_slot: Arc::new(Mutex::new(None)),
            local_handler_depth: Arc::new(AtomicU32::new(0)),
            sys_live: Arc::new(Mutex::new(None)),
            access: None,
            store_conflicts: Arc::new(Mutex::new(None)),
            values_injected: false,
            log_budget: Arc::new(AtomicU32::new(DEFAULT_LUA_LOG_EVENTS)),
            log_byte_budget: Arc::new(AtomicUsize::new(log_byte_budget(DEFAULT_LUA_LOG_EVENTS))),
            local_tools: LocalTools::default(),
            instruction_budget: InstructionBudget::default(),
            raw_model_ids: false,
            #[cfg(feature = "test-support")]
            _vm_tally: tally,
        };
        if let Err(error) = harden(&vm.lua) {
            return vm.construction_failed(error, emitter, section);
        }
        if let Err(error) = install_deterministic_iteration(&vm.lua) {
            return vm.construction_failed(error, emitter, section);
        }
        if let Err(error) = install_untrusted(&vm.lua, nonce) {
            return vm.construction_failed(error, emitter, section);
        }
        match install_instruction_budget(&vm.lua) {
            Ok(budget) => vm.instruction_budget = budget,
            Err(error) => return vm.construction_failed(error, emitter, section),
        }
        Ok(vm)
    }
    /// Creates a section VM sharing the run's tool and model sets.
    ///
    /// The sets are the run's own handles, not snapshots: the offering and
    /// the filled roles back the validating `tools`/`models` tables that
    /// [`inject_values_with_var`](Self::inject_values_with_var) installs,
    /// and the prompt-wide facts a section records (`tools.always_offer`,
    /// `models.default`) land where every later section sees them. H1 is
    /// section 0 on this same path.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM cannot be built or hardened.
    pub fn new_for_section(
        nonce: &GuardNonce,
        tools: &Arc<Mutex<ToolSet>>,
        models: &Arc<Mutex<ModelSet>>,
        emitter: &Emitter,
        section: &str,
    ) -> Result<Self> {
        let mut vm = Self::new(nonce, emitter, section)?;
        vm.bound_tools = Arc::clone(tools);
        vm.bound_models = Arc::clone(models);
        Ok(vm)
    }

    /// Installs the run's cancel flag on this VM's instruction hook: every
    /// block coroutine the VM starts polls it, and a set flag aborts the
    /// running chunk as [`Error::Interrupted`]. A VM without one is never
    /// cancelled. The first install wins.
    pub fn set_cancel(&self, cancel: promptforge_types::cancel::CancelHandle) {
        self.instruction_budget.set_cancel(cancel);
    }

    /// Opts the VM into the raw-model-id fallback, on whenever the caller
    /// passes an application-state snapshot (`RunContext::ui`): `models.get`
    /// resolves an undeclared alias as a raw gateway catalog model id.
    ///
    /// Must be called before [`inject_values_with_var`](Self::inject_values_with_var),
    /// whose H2 `models` table install reads the flag. A run without a
    /// snapshot never calls it, so it keeps strict declared-alias
    /// resolution.
    pub fn allow_raw_model_ids(&mut self) {
        self.raw_model_ids = true;
    }

    /// Applies the run's Lua resource limits to this VM.
    ///
    /// Sets the heap ceiling (`lua_memory_bytes`) and resets the `log()` event
    /// budget (`lua_log_events`). Called by the executor right after
    /// construction, ahead of the shared replay, so the replay already spends
    /// the caller's run limits rather than only the safe non-env defaults
    /// installed in [`SectionVm::new`].
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the underlying VM rejects the memory limit.
    pub fn apply_lua_limits(&self, memory_bytes: usize, log_events: u32) -> Result<()> {
        self.lua
            .set_memory_limit(memory_bytes)
            .map_err(Error::lua)?;
        self.log_budget.store(log_events, Ordering::Relaxed);
        self.log_byte_budget
            .store(log_byte_budget(log_events), Ordering::Relaxed);
        Ok(())
    }

    /// Destroys this section VM at an explicit observed lifecycle boundary.
    ///
    /// The emitter is borrowed only for this synchronous call.
    pub fn teardown(self, emitter: &Emitter, section: &str) {
        emitter.report(section, lifecycle::LUA_TEARDOWN_STARTED);
        drop(self);
        emitter.report(section, lifecycle::LUA_TEARDOWN_SUCCEEDED);
    }

    fn construction_failed(self, error: Error, emitter: &Emitter, section: &str) -> Result<Self> {
        self.teardown(emitter, section);
        Err(error)
    }
}
