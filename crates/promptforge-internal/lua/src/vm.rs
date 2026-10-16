//! The per-section Lua VM: its construction, the typestate that orders
//! Engine injection before every later step, and the local tools it
//! registers. Injection is in `inject`, the later installs in `install`,
//! block coroutines in `run`, and state reads in `state`.
//!
//! A local tool registered with `tools.offer_local` runs inside the calling
//! block's coroutine, never as an Engine function. The VM keeps only each
//! tool's alias and schema; the handler sits in the VM's handler table. A
//! call takes two yields: the shim's `tool_call`, which the scheduler
//! answers with the handler itself, and the shim's `local_tool_done`
//! after the handler ran, which carries its result for the scheduler to
//! report. Every suspending call the handler makes in between is an
//! ordinary yield of the same coroutine.

mod inject;
mod install;
mod run;
mod state;

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{Lua, LuaOptions, StdLib};
use promptforge_model_client::client::ToolSchema;
use promptforge_model_client::model::ModelSet;
use promptforge_types::emitter::Emitter;
use promptforge_types::event::lifecycle;
use promptforge_types::untrusted::GuardNonce;
use promptforge_vfs::Access;
use serde_json::Value as Json;

use crate::alias::Alias;
use crate::engine_globals::{LUA_LOG_CHARACTER_LIMIT, install_untrusted};
use crate::error::{Error, Result};
use crate::handles::ToolSet;
use crate::hardening::{InstructionBudget, harden, install_instruction_budget};
use crate::iteration::install_deterministic_iteration;
use crate::models::{ModelRuntime, RawModelIds};
use crate::program::LuaProgram;
use crate::scope::ToolRuntime;

pub use inject::InjectFailure;
pub use run::CoroStep;
pub(crate) use run::scalar_return;
pub use state::{current_tool_bindings, resolve_model_binding};

/// Default per-VM Lua heap ceiling, matching the executor's `RunLimits`.
pub(crate) const DEFAULT_LUA_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// Default per-VM `log()` event budget, matching the executor's `RunLimits`.
pub(crate) const DEFAULT_LUA_LOG_EVENTS: u32 = 1024;

/// The failure of a `prose` read on a VM whose live `sys` slot was never
/// filled, which Engine injection fills.
pub(crate) const NOT_INJECTED: &str = "section VM Engine values have not been injected";

/// The failure of a poisoned live `sys` slot.
pub(crate) const SYS_LIVE_POISONED: &str = "sys live slot was poisoned";

/// Returns the cumulative `log()` byte ceiling derived from the event budget.
///
/// Bounds total log volume (bytes) even when each event is under the per-event
/// character ceiling. Derived as `events * LUA_LOG_CHARACTER_LIMIT` so it scales
/// with the configured event budget.
fn log_byte_budget(log_events: u32) -> usize {
    (log_events as usize).saturating_mul(LUA_LOG_CHARACTER_LIMIT)
}

/// Packs owned values into a 1-based Lua sequence table.
fn pack_sequence<T: mlua::IntoLua>(lua: &Lua, values: Vec<T>) -> mlua::Result<mlua::Table> {
    let table = lua.create_table_with_capacity(values.len(), 0)?;
    for (index, value) in values.into_iter().enumerate() {
        table.raw_set(index + 1, value)?;
    }
    Ok(table)
}

/// A section VM's stage before Engine injection: construction, the run's
/// limits, the cancel flag, and the raw-model-id opt-in.
#[derive(Debug)]
pub struct Uninjected;

/// A section VM's stage once Engine injection ran, holding the chain
/// step's VFS access capability: the `store` table's closures share it, so
/// every store op is attributed to the identity the executor installed.
#[derive(Debug)]
pub struct Injected {
    access: Arc<Access>,
}

/// One hardened, isolated Lua VM for a section's complete lifecycle.
///
/// The VM owns one Lua environment from construction until drop. Construction
/// hardens the sandbox and installs `untrusted`, giving a
/// [`SectionVm<Uninjected>`]; the caller applies the run's limits and then
/// injects the Engine values, which consumes it and gives the injected VM.
/// Only an injected VM installs the persistent Engine globals, the control
/// globals, and `require`, and walks the section's blocks with
/// [`start_block_coro`](Self::start_block_coro), so no step runs before
/// injection and injection runs once. A single instruction hook covers every
/// program run by this VM, on the main state and on every block coroutine,
/// so cancellation reaches any chunk.
///
/// The underlying [`Lua`] is reachable through `lua`, on the injected VM
/// only, for the section setup's installers and the driver's yield
/// parsing; hardening, Engine injection, instruction accounting, and
/// report delivery stay on the VM's own path. Each section must receive a
/// new instance; dropping it destroys all Lua memory belonging to that
/// section. Once Lua allocation succeeds, construction failures cross the
/// same explicit observed teardown boundary as later lifecycle failures.
#[derive(Debug)]
pub struct SectionVm<S = Injected> {
    lua: Lua,
    /// The run's shared tool set: the declared plugins and the offering.
    /// Shared with the run, not snapshotted; nothing writes it during a
    /// run.
    pub(crate) bound_tools: Arc<ToolSet>,
    /// The run's shared model set: the frontmatter's filled roles plus the
    /// prompt-wide default. Shared for the same reason (`models.default`).
    pub(crate) bound_models: Arc<Mutex<ModelSet>>,
    /// The section's tool-addition runtime, read by the executor's scope path.
    tool_runtime: Arc<Mutex<ToolRuntime>>,
    /// The section's model-selection runtime, read by the executor's scope path.
    model_runtime: Arc<Mutex<ModelRuntime>>,
    /// Set by Lua `jump` before it aborts the current chunk.
    jump_slot: Arc<Mutex<Option<String>>>,
    /// How many local tool handlers are running on this VM; `jump` refuses
    /// while it is above zero.
    local_handler_depth: Arc<AtomicU32>,
    /// How many libraries `require` is loading on this VM; `jump` refuses
    /// while it is above zero.
    library_load_depth: Arc<AtomicU32>,
    /// The blocks of every library `require` loaded into this VM: a block
    /// failure maps through them as well as through the started block, so
    /// library code reports its own location and prompt line.
    library_programs: Arc<Mutex<Vec<LuaProgram>>>,
    /// Live sealed `sys` JSON, mirrored for [`current_sys`](Self::current_sys)
    /// snapshots.
    sys_live: Arc<Mutex<Option<Json>>>,
    /// The slot the direct store closures record a claims-model conflict
    /// into while the Plugin preludes install, for the executor to read
    /// once they return via
    /// [`take_store_conflict`](Self::take_store_conflict).
    store_conflicts: Arc<Mutex<Option<String>>>,
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
    /// The raw-model-id fallback, resolved whenever the caller passes an
    /// application-state snapshot (`RunContext::ui`): then `models.get`
    /// resolves an undeclared alias as a raw gateway catalog model id. Set
    /// by [`allow_raw_model_ids`](SectionVm::allow_raw_model_ids) before
    /// Engine injection; refused everywhere else.
    raw_model_ids: RawModelIds,
    /// The lifecycle stage: [`Uninjected`] or [`Injected`].
    state: S,
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
    entries: Arc<Mutex<Vec<(Alias, ToolSchema)>>>,
}

impl LocalTools {
    /// Locks the registry.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the entries lock was poisoned.
    fn entries(&self) -> Result<std::sync::MutexGuard<'_, Vec<(Alias, ToolSchema)>>> {
        self.entries
            .lock()
            .map_err(|_| Error::Internal("local tools registry was poisoned".into()))
    }

    /// Registers a local tool: alias and prebuilt schema.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the entries lock was poisoned.
    pub(crate) fn register(&self, alias: Alias, schema: ToolSchema) -> Result<()> {
        self.entries()?.push((alias, schema));
        Ok(())
    }

    /// Returns the schemas of every registered local tool.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the entries lock was poisoned.
    pub(crate) fn schemas(&self) -> Result<Vec<ToolSchema>> {
        Ok(self
            .entries()?
            .iter()
            .map(|(_, schema)| schema.clone())
            .collect())
    }

    /// Returns whether `alias` names a registered local tool.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the entries lock was poisoned.
    pub(crate) fn contains(&self, alias: &str) -> Result<bool> {
        Ok(self
            .entries()?
            .iter()
            .any(|(name, _)| name.as_str() == alias))
    }

    #[cfg(test)]
    pub(crate) fn entries_handle(&self) -> Arc<Mutex<Vec<(Alias, ToolSchema)>>> {
        Arc::clone(&self.entries)
    }
}

impl SectionVm<Uninjected> {
    /// Creates a hardened section VM.
    ///
    /// Construction installs only the sandbox (the `_G` guard included),
    /// the deterministic `pairs`/`next` walk, the default resource
    /// ceilings, the instruction hook, and `untrusted` (wrapping under the
    /// run's `nonce`). Everything else - the run's
    /// limits, the Engine values, the persistent Engine globals, the control
    /// globals, and `require` - is a separate explicit step the caller
    /// drives in that order (see the type-level docs). Every
    /// lifecycle report goes through the emitter the caller hands each
    /// step; the VM retains none.
    ///
    /// The VM shares an empty tool and model set, so the validating
    /// `tools.offer` installed by
    /// [`inject_values_with_var`](Self::inject_values_with_var) refuses
    /// every id.
    ///
    /// # Errors
    /// Returns [`Error::LuaRuntime`] if the VM cannot be built or hardened.
    pub fn new(nonce: &GuardNonce, emitter: &Emitter, section: &str) -> Result<Self> {
        let lua = Lua::new_with(
            StdLib::STRING | StdLib::TABLE | StdLib::MATH,
            LuaOptions::default(),
        )
        .map_err(Error::lua)?;
        #[cfg(any(test, feature = "test-support"))]
        crate::test_support::count_live_vm(&lua);
        // Bound the VM heap by default; `apply_lua_limits` may tighten or relax
        // it to the caller's `RunLimits`. A safe non-env default keeps every VM
        // bounded even when the run installs no explicit limits.
        lua.set_memory_limit(DEFAULT_LUA_MEMORY_BYTES)
            .map_err(Error::lua)?;
        let mut vm = Self {
            lua,
            bound_tools: Arc::new(ToolSet::default()),
            bound_models: Arc::new(Mutex::new(ModelSet::default())),
            tool_runtime: Arc::new(Mutex::new(ToolRuntime::default())),
            model_runtime: Arc::new(Mutex::new(ModelRuntime::new())),
            jump_slot: Arc::new(Mutex::new(None)),
            local_handler_depth: Arc::new(AtomicU32::new(0)),
            library_load_depth: Arc::new(AtomicU32::new(0)),
            library_programs: Arc::new(Mutex::new(Vec::new())),
            sys_live: Arc::new(Mutex::new(None)),
            store_conflicts: Arc::new(Mutex::new(None)),
            log_budget: Arc::new(AtomicU32::new(DEFAULT_LUA_LOG_EVENTS)),
            log_byte_budget: Arc::new(AtomicUsize::new(log_byte_budget(DEFAULT_LUA_LOG_EVENTS))),
            local_tools: LocalTools::default(),
            instruction_budget: InstructionBudget::default(),
            raw_model_ids: RawModelIds::Refused,
            state: Uninjected,
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
    /// and the prompt-wide fact a section records (`models.default`) lands
    /// where every later section sees it. Nothing writes the tool set
    /// during a run. H1 is section 0 on this same path.
    ///
    /// # Errors
    /// Returns [`Error::LuaRuntime`] if the VM cannot be built or hardened.
    pub fn new_for_section(
        nonce: &GuardNonce,
        tools: &Arc<ToolSet>,
        models: &Arc<Mutex<ModelSet>>,
        emitter: &Emitter,
        section: &str,
    ) -> Result<Self> {
        let mut vm = Self::new(nonce, emitter, section)?;
        vm.bound_tools = Arc::clone(tools);
        vm.bound_models = Arc::clone(models);
        Ok(vm)
    }

    /// Opts the VM into the raw-model-id fallback, on whenever the caller
    /// passes an application-state snapshot (`RunContext::ui`): `models.get`
    /// resolves an undeclared alias as a raw gateway catalog model id.
    /// Engine injection's `models` table install reads the flag, so only an
    /// uninjected VM takes it. A run without a snapshot never calls it, so
    /// it keeps strict declared-alias resolution.
    pub fn allow_raw_model_ids(&mut self) {
        self.raw_model_ids = RawModelIds::Resolved;
    }

    /// Applies the run's Lua resource limits to this VM.
    ///
    /// Sets the heap ceiling (`lua_memory_bytes`) and resets the `log()` event
    /// budget (`lua_log_events`). Called by the executor right after
    /// construction, ahead of the Plugin preludes and every block, so they
    /// already spend the caller's run limits rather than only the safe
    /// non-env defaults installed in [`SectionVm::new`].
    ///
    /// # Errors
    /// Returns [`Error::LuaRuntime`] if the underlying VM rejects the memory
    /// limit.
    pub fn apply_lua_limits(&self, memory_bytes: usize, log_events: u32) -> Result<()> {
        self.lua
            .set_memory_limit(memory_bytes)
            .map_err(Error::lua)?;
        self.log_budget.store(log_events, Ordering::Relaxed);
        self.log_byte_budget
            .store(log_byte_budget(log_events), Ordering::Relaxed);
        Ok(())
    }

    fn construction_failed(self, error: Error, emitter: &Emitter, section: &str) -> Result<Self> {
        self.teardown(emitter, section);
        Err(error)
    }

    /// Borrows the inner Lua state before injection, for the test that
    /// shows construction already installs `untrusted`.
    #[cfg(test)]
    pub(crate) fn uninjected_lua(&self) -> &Lua {
        &self.lua
    }
}

impl<S> SectionVm<S> {
    /// Installs the run's cancel flag on this VM's instruction hook: every
    /// block coroutine the VM starts polls it, and a set flag aborts the
    /// running chunk as [`Error::Interrupted`]. A VM without one is never
    /// cancelled. The first install wins.
    pub fn set_cancel(&self, cancel: promptforge_types::cancel::CancelHandle) {
        self.instruction_budget.set_cancel(cancel);
    }

    /// The section's tool-addition runtime: what `tools.offer`,
    /// `plugins.offer`, and `tools.allow_tasks` recorded.
    #[must_use]
    pub fn tool_runtime(&self) -> &Arc<Mutex<ToolRuntime>> {
        &self.tool_runtime
    }

    /// The section's model-selection runtime: what `models.use` recorded.
    #[must_use]
    pub fn model_runtime(&self) -> &Arc<Mutex<ModelRuntime>> {
        &self.model_runtime
    }

    /// Destroys this section VM at an explicit observed lifecycle boundary.
    ///
    /// The emitter is borrowed only for this synchronous call.
    pub fn teardown(self, emitter: &Emitter, section: &str) {
        emitter.report(section, lifecycle::LUA_TEARDOWN_STARTED);
        drop(self);
        emitter.report(section, lifecycle::LUA_TEARDOWN_SUCCEEDED);
    }
}

impl SectionVm {
    /// Borrows the inner Lua state, for the section setup's installers
    /// (the `ui` global, the loop and store shims, the Plugin preludes) and
    /// the driver's yield parsing and answer rendering.
    #[must_use]
    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}
