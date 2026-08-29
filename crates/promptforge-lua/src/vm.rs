#[cfg(test)]
use super::LuaFanoutResult;
use super::{
    Arc, AtomicU32, AtomicUsize, BTreeMap, DEFAULT_LUA_LOG_EVENTS, DEFAULT_LUA_MEMORY_BYTES, Error,
    Function, GuardNonce, InstructionBudget, IntoLuaMulti, Json, Lua, LuaBlockResult,
    LuaModelHandle, LuaOptions, LuaProgram, LuaSerdeExt, LuaToolHandle, ModelBinding,
    ModelInferHook, ModelRuntime, ModelSet, ModelView, ModelsInferHook, MultiValue, Mutex,
    Observer, Ordering, Result, StdLib, StoreRef, Thread, ThreadStatus, ToolBinding,
    ToolCallCounts, ToolRuntime, ToolSet, Value, WriteScope, detail, guarded_var, harden,
    install_h2_models, install_h2_tools, install_instruction_budget, install_log,
    install_lua_tool_calls, install_shim_prelude, install_store_table, install_untrusted,
    log_byte_budget, resolve_section_target, scalar_return, seal_sys, var_to_json,
    wrap_shimmed_handle,
};
use promptforge_gateway_client::client::ToolSchema;

use crate::protocol::{Answer, Request, YieldParse};

/// Packs owned values into a 1-based Lua sequence table.
pub(crate) fn pack_sequence<T: mlua::IntoLua>(
    lua: &Lua,
    values: Vec<T>,
) -> mlua::Result<mlua::Table> {
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
/// linear startup: apply the run's limits, inject the host values, install
/// the persistent host APIs and the control globals, replay the shared
/// library as the section's first chunk
/// ([`replay_shared`](Self::replay_shared)), install the captured tool/model
/// alias globals, and only then walk the section's blocks with
/// [`start_block_coro`](Self::start_block_coro). [`bind_reply`](Self::bind_reply) inserts
/// the model reply into the same environment between chunks. A single
/// instruction counter covers every program run by this VM, so splitting
/// work across chunks cannot reset the budget.
///
/// `SectionVm` deliberately does not expose its underlying [`Lua`]. This keeps
/// hardening, host injection, instruction accounting, and report delivery on
/// the one owned path. Each section must receive a new instance; dropping it
/// destroys all Lua memory belonging to that section. Once Lua allocation
/// succeeds, construction, shared-load, and captured-binding failures cross
/// the same explicit observed teardown boundary as later lifecycle failures.
///
/// # Examples
/// ```text
/// use promptforge_lua::SectionVm;
/// use promptforge_core_support::observe::NullObserver;
/// use promptforge_core_support::untrusted::GuardNonce;
///
/// let nonce = GuardNonce::fresh();
/// let vm = SectionVm::new(&nonce, "example-run", &NullObserver::default(), "Example")?;
/// vm.teardown(&NullObserver::default(), "Example");
/// # Ok::<(), promptforge_lua::Error>(())
/// ```
#[derive(Debug)]
pub struct SectionVm {
    execution: String,
    lua: Lua,
    bound_tools: ToolSet,
    bound_models: ModelSet,
    /// The section's tool-addition runtime, read by the executor's prose path.
    pub tool_runtime: Arc<Mutex<ToolRuntime>>,
    /// The section's model-selection runtime, read by the executor's prose path.
    pub model_runtime: Arc<Mutex<ModelRuntime>>,
    /// Set by Lua `jump` before it aborts the current chunk.
    jump_slot: Arc<Mutex<Option<String>>>,
    /// Live sealed `sys` JSON, mirrored for [`current_sys`](Self::current_sys)
    /// snapshots.
    sys_live: Arc<Mutex<Option<Json>>>,
    store: Option<StoreRef>,
    /// The fanout arm's write scope for `store.write`; `None` outside an arm
    /// leaves walk-section writes untracked.
    write_scope: Option<WriteScope>,
    host_injected: bool,
    /// Remaining `log()` events this VM may emit before the budget is exhausted.
    log_budget: Arc<AtomicU32>,
    /// Remaining cumulative `log()` message bytes this VM may emit. Bounds total
    /// log volume even when each event is under the per-event ceilings.
    log_byte_budget: Arc<AtomicUsize>,
    /// Local tools registered by Lua code, dispatched back into this VM.
    local_tools: LocalTools,
    /// Set by [`install_coro_shims`](Self::install_coro_shims): the captured
    /// model alias globals install as shim-wrapped proxy tables so
    /// `handle:infer` yields like `models.infer`.
    coro_shims: bool,
    /// The VM's instruction-budget counter, shared with every block
    /// coroutine's hook (hooks are per-coroutine in PUC Lua).
    instruction_budget: InstructionBudget,
}

/// Local tool registrations owned by a section VM.
///
/// Each entry holds the tool alias, its prebuilt schema, and the registry key
/// for the Lua handler function captured at registration time. The entries are
/// shared with the `tools.add_local` Lua callback, which must be `Send`, hence the
/// `Mutex`; the VM is single-threaded, so the lock never contends.
#[derive(Debug, Default, Clone)]
pub(crate) struct LocalTools {
    entries: Arc<Mutex<Vec<(String, ToolSchema, mlua::RegistryKey)>>>,
}

impl LocalTools {
    /// Registers a local tool: alias, prebuilt schema, and the registry key
    /// of the Lua handler function.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the entries lock was poisoned.
    pub(crate) fn register(
        &self,
        alias: String,
        schema: ToolSchema,
        handler: mlua::RegistryKey,
    ) -> Result<()> {
        self.entries
            .lock()
            .map_err(|_| Error::Lua("local tools registry was poisoned".to_owned()))?
            .push((alias, schema, handler));
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
            .map(|(_, schema, _)| schema.clone())
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
            .any(|(name, _, _)| name == alias))
    }

    #[cfg(test)]
    pub(crate) fn entries_handle(
        &self,
    ) -> Arc<Mutex<Vec<(String, ToolSchema, mlua::RegistryKey)>>> {
        Arc::clone(&self.entries)
    }

    /// Calls the handler registered under `alias` with JSON `args`.
    ///
    /// The `jump` global is nilled for the handler's duration and restored
    /// afterward: a local tool runs outside any chunk's control flow, so a
    /// jump recorded here would surface stale at the next chunk boundary.
    /// Handlers cannot call the suspending shims (`execute`, `fanout`,
    /// `models.infer`): the handler runs outside any coroutine, so the
    /// shim's yield fails with "attempt to yield from outside a coroutine".
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if no local tool is registered under `alias`,
    /// the args cannot be bridged, the jump guard cannot be applied or
    /// restored, the handler fails, or it returns a non-scalar value.
    pub(crate) fn call(&self, lua: &Lua, alias: &str, args: &Json) -> Result<String> {
        let handler: Function = {
            let entries = self
                .entries
                .lock()
                .map_err(|_| Error::Lua("local tools registry was poisoned".to_owned()))?;
            let key = entries
                .iter()
                .find(|(name, _, _)| name == alias)
                .map(|(_, _, key)| key)
                .ok_or_else(|| Error::Lua(format!("local tool {alias:?} is not registered")))?;
            lua.registry_value(key).map_err(Error::lua)?
        };
        let table = lua.to_value(args).map_err(Error::lua)?;
        let globals = lua.globals();
        let saved_jump: Value = globals.raw_get("jump").map_err(Error::lua)?;
        globals.raw_set("jump", Value::Nil).map_err(Error::lua)?;
        let returned = handler.call(table);
        // Restore even on handler failure; a restore failure on top of a
        // handler failure reports the handler's error, which came first.
        let restore = globals.raw_set("jump", saved_jump).map_err(Error::lua);
        let returned: MultiValue = match (returned, restore) {
            (Ok(values), Ok(())) => values,
            (Err(error), _) => return Err(Error::lua(error)),
            (Ok(_), Err(error)) => return Err(error),
        };
        Ok(scalar_return(returned)?.unwrap_or_default())
    }
}

impl SectionVm {
    /// Creates a hardened section VM.
    ///
    /// Construction installs only the sandbox, the default resource ceilings,
    /// the instruction budget, and `untrusted` (wrapping under the run's
    /// `nonce`). Everything else - the run's
    /// limits, the host values, the persistent host APIs, the control
    /// globals, the shared-library replay, and the captured alias globals -
    /// is a separate explicit step the caller drives in that order (see the
    /// type-level docs). The VM retains `execution` for every later
    /// lifecycle report.
    ///
    /// The VM carries no frozen tool bindings, so the validating `tools.add`
    /// installed by [`inject_host`](Self::inject_host) rejects every alias as
    /// undeclared: a prompt without `tools.bind` declarations cannot scope
    /// tools.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM cannot be built or hardened.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_lua::SectionVm;
    /// use promptforge_core_support::observe::NullObserver;
    /// use promptforge_core_support::untrusted::GuardNonce;
    ///
    /// let nonce = GuardNonce::fresh();
    /// let vm = SectionVm::new(&nonce, "example-run", &NullObserver::default(), "Example")?;
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_lua::Error>(())
    /// ```
    pub fn new(
        nonce: &GuardNonce,
        execution: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
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
            execution: execution.to_owned(),
            lua,
            bound_tools: ToolSet::default(),
            bound_models: ModelSet::default(),
            tool_runtime: Arc::new(Mutex::new(ToolRuntime {
                added: Vec::new(),
                description_overrides: BTreeMap::new(),
            })),
            model_runtime: Arc::new(Mutex::new(ModelRuntime::new())),
            jump_slot: Arc::new(Mutex::new(None)),
            sys_live: Arc::new(Mutex::new(None)),
            store: None,
            write_scope: None,
            host_injected: false,
            log_budget: Arc::new(AtomicU32::new(DEFAULT_LUA_LOG_EVENTS)),
            log_byte_budget: Arc::new(AtomicUsize::new(log_byte_budget(DEFAULT_LUA_LOG_EVENTS))),
            local_tools: LocalTools::default(),
            coro_shims: false,
            instruction_budget: InstructionBudget::default(),
        };
        if let Err(error) = harden(&vm.lua) {
            return vm.construction_failed(error, observer, section);
        }
        if let Err(error) = install_untrusted(&vm.lua, nonce) {
            return vm.construction_failed(error, observer, section);
        }
        match install_instruction_budget(&vm.lua) {
            Ok(budget) => vm.instruction_budget = budget,
            Err(error) => return vm.construction_failed(error, observer, section),
        }
        Ok(vm)
    }
    /// Creates a section VM carrying the prompt's frozen tool and model bindings.
    ///
    /// The bindings back the validating `tools`/`models` tables that
    /// [`inject_host_with_var`](Self::inject_host_with_var) installs, and the
    /// bare alias globals that
    /// [`install_captured_bindings`](Self::install_captured_bindings)
    /// installs after the shared replay. H1 code is never replayed into a
    /// section VM.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM cannot be built or hardened.
    pub fn new_for_section(
        nonce: &GuardNonce,
        tools: &ToolSet,
        models: &ModelSet,
        execution: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        let mut vm = Self::new(nonce, execution, observer, section)?;
        vm.bound_tools = tools.clone();
        vm.bound_models = models.clone();
        Ok(vm)
    }

    /// Replays the shared library as the section's first chunk.
    ///
    /// The replay runs through the normal chunk path with the full host
    /// environment already installed: `args`, `sys`, `var`, `reply`, `log`,
    /// `store`, the `tools`/`models` tables, and the control globals are all
    /// visible to shared top-level code. Only the captured tool/model alias
    /// globals are absent; they install afterward via
    /// [`install_captured_bindings`](Self::install_captured_bindings) so a
    /// declared alias wins over a same-named shared global. A scalar
    /// top-level return is discarded: the replay is a library load, not a
    /// result.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the shared program fails or returns a
    /// non-scalar value, or if it calls `jump`: load-time control transfer
    /// has no coherent meaning, so a recorded jump becomes the hard error
    /// "jump is not available during shared library load".
    pub fn replay_shared(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<()> {
        observer.observe(&self.execution, section, detail::LUA_SHARED_LOAD_STARTED);
        match self.run_loaded_with_control(program) {
            Ok(LuaBlockResult::Returned(_)) => {
                observer.observe(&self.execution, section, detail::LUA_SHARED_LOAD_SUCCEEDED);
                Ok(())
            }
            Ok(LuaBlockResult::Jump(_)) => {
                observer.observe(&self.execution, section, detail::LUA_SHARED_LOAD_FAILED);
                Err(Error::Lua(
                    "jump is not available during shared library load".to_owned(),
                ))
            }
            Err(error) => {
                observer.observe(&self.execution, section, detail::LUA_SHARED_LOAD_FAILED);
                Err(error)
            }
        }
    }

    /// Installs the captured tool and model alias globals.
    ///
    /// Each frozen binding becomes a bare global holding its handle userdata.
    /// The engine calls this after [`replay_shared`](Self::replay_shared), so
    /// a declared alias wins over a same-named shared global; the raw install
    /// also bypasses any metatable the shared library set on `_G`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if a handle cannot be created or installed.
    pub fn install_captured_bindings(&self) -> Result<()> {
        let globals = self.lua.globals();
        for binding in self.bound_tools.bindings() {
            let handle =
                LuaToolHandle::from_binding(binding.alias(), binding.description(), binding.id());
            let userdata = self.lua.create_userdata(handle).map_err(Error::lua)?;
            globals
                .raw_set(binding.alias(), userdata)
                .map_err(Error::lua)?;
        }
        for binding in self.bound_models.bindings() {
            let handle = LuaModelHandle::from_binding(binding);
            // Scheduler mode: the alias installs as a shim-wrapped proxy so
            // `handle:infer` yields like `models.infer`; legacy mode keeps
            // the bare userdata.
            let value = if self.coro_shims {
                wrap_shimmed_handle(&self.lua, handle)?
            } else {
                Value::UserData(self.lua.create_userdata(handle).map_err(Error::lua)?)
            };
            globals
                .raw_set(binding.alias(), value)
                .map_err(Error::lua)?;
        }
        Ok(())
    }

    /// Installs the section's host values, ahead of the shared replay.
    ///
    /// This operation may be called exactly once. The store callbacks own a
    /// clone of the run-scoped store. `log` and `store` are installed once for
    /// the section's whole lifecycle by
    /// [`install_host_apis`](Self::install_host_apis), which captures an
    /// observer `Arc` rather than a per-chunk borrow.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values cannot be bridged or if host
    /// values were already injected.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_lua::SectionVm;
    /// use promptforge_core_support::observe::NullObserver;
    /// use promptforge_store::StoreRef;
    /// use promptforge_core_support::untrusted::GuardNonce;
    ///
    /// let nonce = GuardNonce::fresh();
    /// let mut vm = SectionVm::new(&nonce, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("input", &serde_json::json!({ "id": 1 }), &StoreRef::memory(), None)?;
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_lua::Error>(())
    /// ```
    pub fn inject_host(
        &mut self,
        args: &str,
        sys: &Json,
        store: &StoreRef,
        last_reply: Option<&str>,
    ) -> Result<()> {
        self.inject_host_with_var(args, sys, store, last_reply, None, None)
    }

    /// Installs host values while seeding `var` from an earlier VM.
    ///
    /// The `var` global is a guarded proxy (see [`guarded_var`]): writes are
    /// validated for JSON-representability at the assigning line, and the
    /// hidden data table behind it is what [`var`](Self::var) reads back.
    /// `write_scope` is the fanout arm's store-write identity; it is `None`
    /// for every other driver, leaving `store.write` untracked.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values cannot be bridged or were already
    /// injected.
    pub fn inject_host_with_var(
        &mut self,
        args: &str,
        sys: &Json,
        store: &StoreRef,
        last_reply: Option<&str>,
        initial_var: Option<&Json>,
        write_scope: Option<WriteScope>,
    ) -> Result<()> {
        if self.host_injected {
            return Err(Error::Lua(
                "section VM host values were already injected".to_owned(),
            ));
        }

        let globals = self.lua.globals();
        globals.raw_set("args", args).map_err(Error::lua)?;
        let sys_table = seal_sys(&self.lua, sys)?;
        globals.raw_set("sys", sys_table).map_err(Error::lua)?;
        {
            let mut live = self
                .sys_live
                .lock()
                .map_err(|_| Error::Lua("sys live slot was poisoned".to_owned()))?;
            *live = Some(sys.clone());
        }
        let var = guarded_var(&self.lua, initial_var)?;
        globals.raw_set("var", var).map_err(Error::lua)?;
        install_h2_tools(
            &self.lua,
            &globals,
            &self.bound_tools,
            &self.tool_runtime,
            &self.local_tools,
        )?;
        install_h2_models(&self.lua, &globals, &self.bound_models, &self.model_runtime)?;
        let reply_value = match last_reply {
            Some(text) => Value::String(self.lua.create_string(text).map_err(Error::lua)?),
            None => Value::Nil,
        };
        globals.raw_set("reply", reply_value).map_err(Error::lua)?;
        self.store = Some(store.clone());
        self.write_scope = write_scope;
        self.host_injected = true;
        Ok(())
    }

    /// Installs `log` and `store` as persistent globals for the section's
    /// whole lifecycle.
    ///
    /// Called once after [`inject_host_with_var`](Self::inject_host_with_var).
    /// The closures capture owned strings and Arc clones of the observer, the
    /// log budget counters, and the store handle, so they stay valid across
    /// every chunk this VM runs without a live [`mlua::Scope`].
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected or the
    /// globals cannot be installed.
    pub fn install_host_apis(&self, observer: &Arc<dyn Observer>, section: &str) -> Result<()> {
        let store = self.store.as_ref().ok_or_else(|| {
            Error::Lua("section VM host values have not been injected".to_owned())
        })?;
        install_log(
            &self.lua,
            &self.execution,
            observer,
            section,
            &self.log_budget,
            &self.log_byte_budget,
        )?;
        install_store_table(
            &self.lua,
            &self.lua.globals(),
            store,
            &self.execution,
            observer,
            section,
            self.write_scope,
        )
    }

    /// Installs `execute`, `jump`, `fanout`, and `list_from_section` as
    /// persistent globals for the section's whole lifecycle.
    ///
    /// Called once by the engine after host injection. The callbacks own
    /// their run context, so the closures stay valid across every chunk this
    /// VM runs without a live [`mlua::Scope`]. The `jump` closure captures a
    /// clone of the VM's jump slot; the slot is reset before each chunk and
    /// read after it by the control-run path. The `execute` and `fanout`
    /// closures snapshot this VM's `var` at call time (reading the hidden
    /// data table through the in-scope `&Lua`) and hand the JSON to their
    /// callback, so a contained chain or arm seeds from a clone and its
    /// writes never reach this VM.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if any global cannot be installed.
    #[cfg(test)]
    pub(crate) fn install_control_globals<E, F, L>(
        &self,
        execute_callback: E,
        fanout_callback: F,
        list_callback: L,
    ) -> Result<()>
    where
        E: Fn(Value, Option<String>, Json) -> std::result::Result<String, Error> + Send + 'static,
        F: Fn(String, Vec<Json>, Json) -> std::result::Result<Vec<LuaFanoutResult>, Error>
            + Send
            + 'static,
        L: Fn(String) -> std::result::Result<Vec<String>, Error> + Send + 'static,
    {
        let globals = self.lua.globals();
        let execute_fn = self
            .lua
            .create_function(move |lua, (target, input): (Value, Option<String>)| {
                let var = var_to_json(lua).map_err(mlua::Error::external)?;
                execute_callback(target, input, var).map_err(mlua::Error::external)
            })
            .map_err(Error::lua)?;
        globals.raw_set("execute", execute_fn).map_err(Error::lua)?;
        self.install_jump_global(&globals)?;
        let fanout_fn = self
            .lua
            .create_function(move |lua, (worker, collection): (String, Value)| {
                let items = crate::collection::collection_to_items(lua, &collection)
                    .map_err(mlua::Error::external)?;
                let var = var_to_json(lua).map_err(mlua::Error::external)?;
                let replies = fanout_callback(worker, items, var).map_err(mlua::Error::external)?;
                pack_sequence(lua, replies)
            })
            .map_err(Error::lua)?;
        globals.raw_set("fanout", fanout_fn).map_err(Error::lua)?;
        self.install_list_global(&globals, list_callback)
    }

    /// Installs the scheduler-mode control surface: `jump` and
    /// `list_from_section` as Rust callbacks (neither suspends).
    ///
    /// The suspending calls (`models.infer`, `handle:infer`, `execute`,
    /// `fanout`) are the yield shims installed by
    /// [`install_coro_shims`](Self::install_coro_shims).
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if any global cannot be installed.
    pub fn install_scheduler_control_globals<L, E>(&self, list_callback: L) -> Result<()>
    where
        L: Fn(String) -> std::result::Result<Vec<String>, E> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let globals = self.lua.globals();
        self.install_jump_global(&globals)?;
        self.install_list_global(&globals, list_callback)
    }

    /// Installs the coroutine yield shims (`models.infer`, `handle:infer`,
    /// `execute`) and marks the VM so the captured model alias globals
    /// install as shim-wrapped proxies.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the shim prelude cannot install.
    pub fn install_coro_shims(&mut self) -> Result<()> {
        install_shim_prelude(&self.lua)?;
        self.coro_shims = true;
        Ok(())
    }

    fn install_jump_global(&self, globals: &mlua::Table) -> Result<()> {
        let jump_slot = Arc::clone(&self.jump_slot);
        let jump_fn = self
            .lua
            .create_function(move |_, target: Value| -> mlua::Result<()> {
                let heading = resolve_section_target(target)?;
                let mut slot = jump_slot
                    .lock()
                    .map_err(|_| mlua::Error::external("jump slot poisoned"))?;
                *slot = Some(heading);
                Err(mlua::Error::external("jump transfer"))
            })
            .map_err(Error::lua)?;
        globals.raw_set("jump", jump_fn).map_err(Error::lua)
    }

    fn install_list_global<L, E>(&self, globals: &mlua::Table, list_callback: L) -> Result<()>
    where
        L: Fn(String) -> std::result::Result<Vec<String>, E> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let list_fn = self
            .lua
            .create_function(move |lua, target: Value| {
                let heading = resolve_section_target(target)?;
                let items = list_callback(heading).map_err(mlua::Error::external)?;
                pack_sequence(lua, items)
            })
            .map_err(Error::lua)?;
        globals
            .raw_set("list_from_section", list_fn)
            .map_err(Error::lua)
    }

    /// Installs `execute`, `jump`, `fanout`, and `list_from_section` as
    /// stubs that fail with a clear error, for the live H1 VM only.
    ///
    /// H1 runs before any section exists, so the real control globals can
    /// never operate there; without stubs a call dies with Lua's stock
    /// nil-call error, which names no cause.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if any global cannot be installed.
    pub fn install_h1_control_stubs(&self) -> Result<()> {
        let globals = self.lua.globals();
        for name in ["execute", "jump", "fanout", "list_from_section"] {
            let stub = self
                .lua
                .create_function(move |_, _: MultiValue| -> mlua::Result<()> {
                    Err(mlua::Error::external(format!(
                        "{name} is only available in sections (## headings); H1 runs before sections exist"
                    )))
                })
                .map_err(Error::lua)?;
            globals.raw_set(name, stub).map_err(Error::lua)?;
        }
        Ok(())
    }

    /// Replaces the sealed Lua `sys` global after scope close.
    ///
    /// Host injection must have run first. Used to expose `sys.model` once the
    /// section's model binding is fixed.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected or the
    /// sealed table cannot be installed.
    pub fn re_seal_sys(&self, sys: &Json) -> Result<()> {
        if !self.host_injected {
            return Err(Error::Lua(
                "section VM host values were not injected".to_owned(),
            ));
        }
        let globals = self.lua.globals();
        let sys_table = seal_sys(&self.lua, sys)?;
        globals.raw_set("sys", sys_table).map_err(Error::lua)?;
        let mut live = self
            .sys_live
            .lock()
            .map_err(|_| Error::Lua("sys live slot was poisoned".to_owned()))?;
        *live = Some(sys.clone());
        Ok(())
    }

    /// Shared live `sys` JSON for finish-reason updates.
    #[must_use]
    #[allow(dead_code)] // exercised by the lua module's poisoned-slot test
    pub(crate) fn sys_live_handle(&self) -> Arc<Mutex<Option<Json>>> {
        Arc::clone(&self.sys_live)
    }

    /// Snapshot of the live sealed `sys` JSON, or `fallback` when unset.
    ///
    /// Distinguishes the two non-value states rather than collapsing both to
    /// `fallback`: an *unset* live slot (before any [`Self::re_seal_sys`]) is a
    /// legitimate state and yields `Ok(fallback)`, while a *poisoned* lock is a
    /// real failure and yields [`Error::Lua`] instead of silently masquerading
    /// as the fallback.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the live `sys` mutex is poisoned.
    pub fn current_sys(&self, fallback: &Json) -> Result<Json> {
        let guard = self
            .sys_live
            .lock()
            .map_err(|_| Error::Lua("sys live slot was poisoned".to_owned()))?;
        Ok(guard.clone().unwrap_or_else(|| fallback.clone()))
    }

    /// Executes a compiled Lua chunk in this VM's persistent environment.
    ///
    /// This is the legacy engine's path for running a section's Lua blocks;
    /// the scheduler drives blocks through
    /// [`start_block_coro`](Self::start_block_coro) instead. StoreRef and
    /// `log` reports go to the observer captured by
    /// [`install_host_apis`](Self::install_host_apis); a nil or absent
    /// top-level return produces [`LuaBlockResult::Returned`]`(None)`. When
    /// the chunk may call `execute`, `jump`, or `fanout`, those must
    /// already be installed by
    /// [`install_control_globals`](Self::install_control_globals).
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, execution
    /// fails, the shared instruction budget is exhausted, or the program
    /// returns a non-scalar value.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-core`'s executor
    /// tests, not host API.
    #[doc(hidden)]
    pub fn run_chunk(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<LuaBlockResult> {
        observer.observe(&self.execution, section, detail::LUA_CHUNK_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_CHUNK_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_control(program);
        observer.observe(
            &self.execution,
            section,
            if result.is_ok() {
                detail::LUA_CHUNK_SUCCEEDED
            } else {
                detail::LUA_CHUNK_FAILED
            },
        );
        result
    }

    /// Binds the model reply for later chunks in the same environment.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected or the
    /// reply cannot be installed.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_lua::SectionVm;
    /// use promptforge_core_support::observe::NullObserver;
    /// use promptforge_store::StoreRef;
    /// use promptforge_core_support::untrusted::GuardNonce;
    ///
    /// let nonce = GuardNonce::fresh();
    /// let mut vm = SectionVm::new(&nonce, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// vm.bind_reply("model answer", &NullObserver::default(), "Example")?;
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_lua::Error>(())
    /// ```
    pub fn bind_reply(&self, reply: &str, observer: &dyn Observer, section: &str) -> Result<()> {
        observer.observe(&self.execution, section, detail::LUA_REPLY_BINDING_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_REPLY_BINDING_FAILED);
            return Err(error);
        }
        let result = self
            .lua
            .globals()
            .raw_set("reply", reply)
            .map_err(Error::lua);
        observer.observe(
            &self.execution,
            section,
            if result.is_ok() {
                detail::LUA_REPLY_BINDING_SUCCEEDED
            } else {
                detail::LUA_REPLY_BINDING_FAILED
            },
        );
        result
    }

    /// Reads the Lua `reply` global back into Rust: `None` when nil, the
    /// string when set.
    ///
    /// The global is seeded at [`inject_host`](Self::inject_host) and rebound
    /// after prose by [`bind_reply`](Self::bind_reply), so after a Lua chunk
    /// it carries any author write: `reply = nil` clears the reply a jump
    /// target or the next section sees, and a string assignment replaces it.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when `reply` is neither nil nor a string.
    pub fn reply(&self) -> Result<Option<String>> {
        let value: Value = self.lua.globals().get("reply").map_err(Error::lua)?;
        match value {
            Value::Nil => Ok(None),
            Value::String(text) => Ok(Some(text.to_str().map_err(Error::lua)?.to_owned())),
            other => Err(Error::Lua(format!(
                "`reply` must be a string or nil, got {}",
                other.type_name()
            ))),
        }
    }

    /// Returns the current `var` table as JSON, read from the hidden data
    /// table behind the guarded proxy (not the proxy, which stays empty).
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected or `var`
    /// cannot be represented as JSON.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_lua::SectionVm;
    /// use promptforge_core_support::observe::NullObserver;
    /// use promptforge_store::StoreRef;
    /// use promptforge_core_support::untrusted::GuardNonce;
    ///
    /// let nonce = GuardNonce::fresh();
    /// let mut vm = SectionVm::new(&nonce, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// assert_eq!(vm.var()?, serde_json::json!({}));
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_lua::Error>(())
    /// ```
    pub fn var(&self) -> Result<Json> {
        if !self.host_injected {
            return Err(Error::Lua(
                "section VM host values have not been injected".to_owned(),
            ));
        }
        var_to_json(&self.lua)
    }

    /// Reads a bare global for prose substitution: `None` when the global is
    /// unset, its JSON form when set.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the global is a function, userdata, or
    /// thread (bare globals in prose must be data), or when its value cannot
    /// be represented as JSON.
    pub fn global_json(&self, name: &str) -> Result<Option<Json>> {
        let value: Value = self.lua.globals().get(name).map_err(Error::lua)?;
        match value {
            Value::Nil => Ok(None),
            Value::Function(_) | Value::UserData(_) | Value::Thread(_) => Err(Error::Lua(format!(
                "global `{name}` is a {}; bare globals in prose must be JSON data",
                value.type_name()
            ))),
            other => Ok(Some(self.lua.from_value(other).map_err(Error::lua)?)),
        }
    }

    /// Sets a string global in the VM, overwriting any existing value.
    ///
    /// Used by the H1 path to inject `reply` after host injection.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the global cannot be set.
    pub fn set_global_string(&self, name: &str, value: &str) -> Result<()> {
        self.lua.globals().raw_set(name, value).map_err(Error::lua)
    }

    /// Sets a global in the VM to the Lua form of a JSON value, overwriting
    /// any existing value.
    ///
    /// Used by fanout to inject `item` after host injection; the conversion
    /// is the same `LuaSerdeExt` bridge that seeds `var`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the value cannot convert or the global
    /// cannot be set.
    pub fn set_global_json(&self, name: &str, value: &Json) -> Result<()> {
        let value = self.lua.to_value(value).map_err(Error::lua)?;
        self.lua.globals().raw_set(name, value).map_err(Error::lua)
    }

    /// Installs `tools.calls` as a read-only Lua table backed by a fresh
    /// [`ToolCallCounts`]. Each in-scope alias reads its live count; indexing
    /// an unknown key is a hard error that names the bad key and lists the
    /// in-scope set. When the key was declared by `tools.bind` but not added
    /// to this section's scope, the diagnostic says so.
    ///
    /// Returns the `ToolCallCounts` handle so the executor's tool loop can
    /// increment it.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when installing the `tools.calls` index fails.
    pub fn install_tool_call_counts(&self, bindings: &[ToolBinding]) -> Result<ToolCallCounts> {
        let counts = ToolCallCounts::new(bindings.iter().map(|b| b.alias().to_owned()));
        let declared: Vec<String> = self
            .bound_tools
            .bindings()
            .iter()
            .map(|binding| binding.alias().to_owned())
            .collect();
        install_lua_tool_calls(&self.lua, &counts, &declared)?;
        Ok(counts)
    }

    /// Returns frozen tool bindings and the live H2 addition runtime.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-core`'s executor
    /// tests, not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn tool_bag_handles(&self) -> (ToolSet, Arc<Mutex<ToolRuntime>>) {
        (self.bound_tools.clone(), Arc::clone(&self.tool_runtime))
    }

    /// Returns frozen model bindings and the live H2 selection runtime.
    ///
    /// Test-only: production reads the run's shared set through the model
    /// view; tests snapshot straight from the VM.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-core`'s tests,
    /// not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn model_bag_handles(&self) -> (ModelSet, Arc<Mutex<ModelRuntime>>) {
        (self.bound_models.clone(), Arc::clone(&self.model_runtime))
    }

    /// Borrows the inner Lua state, so the shim installs and the
    /// scheduler's scoped H1 steps can drive coroutines on the VM.
    #[must_use]
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Calls the local tool registered under `alias` with JSON `args`.
    ///
    /// The handler is fetched from the Lua registry, invoked with the args
    /// converted to a Lua table, and its scalar return value is rendered as a
    /// string. A nil return yields an empty string. The `jump` global is
    /// nilled for the handler's duration and restored afterward (see
    /// [`LocalTools::call`]).
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if no local tool is registered under `alias`,
    /// the args cannot be bridged, the handler fails, or it returns a
    /// non-scalar value.
    pub fn call_local_tool(&self, alias: &str, args: &Json) -> Result<String> {
        self.local_tools.call(&self.lua, alias, args)
    }

    /// Returns the schemas of every registered local tool.
    /// # Errors
    /// Returns [`Error::Lua`] if the local-tools registry was poisoned.
    pub fn local_tool_schemas(&self) -> Result<Vec<ToolSchema>> {
        self.local_tools.schemas()
    }

    /// Returns whether `alias` names a registered local tool.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the local-tools registry was poisoned.
    #[allow(dead_code)] // wired up by the local-tools dispatch step
    pub(crate) fn has_local_tool(&self, alias: &str) -> Result<bool> {
        self.local_tools.contains(alias)
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

    /// Clears the `model:infer` and `models.infer` host hooks.
    pub(crate) fn clear_infer_hook(&self) {
        let _ = self.lua.remove_app_data::<ModelInferHook>();
        let _ = self.lua.remove_app_data::<ModelsInferHook>();
    }

    /// Destroys this section VM at an explicit observed lifecycle boundary.
    ///
    /// The observer is borrowed only for this synchronous call and is not
    /// retained by the VM.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_lua::SectionVm;
    /// use promptforge_core_support::observe::NullObserver;
    /// use promptforge_core_support::untrusted::GuardNonce;
    ///
    /// let nonce = GuardNonce::fresh();
    /// let vm = SectionVm::new(&nonce, "example-run", &NullObserver::default(), "Example")?;
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_lua::Error>(())
    /// ```
    pub fn teardown(self, observer: &dyn Observer, section: &str) {
        let execution = self.execution.clone();
        observer.observe(&self.execution, section, detail::LUA_TEARDOWN_STARTED);
        self.clear_infer_hook();
        drop(self);
        observer.observe(&execution, section, detail::LUA_TEARDOWN_SUCCEEDED);
    }

    fn construction_failed(
        self,
        error: Error,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        self.teardown(observer, section);
        Err(error)
    }

    /// Takes any recorded jump target, propagating a poisoned jump-slot lock
    /// rather than silently coercing the failure into "no jump"
    /// (source-audit discarded-error-001).
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the jump-slot mutex is poisoned.
    fn take_jump(&self) -> Result<Option<String>> {
        let mut slot = self
            .jump_slot
            .lock()
            .map_err(|_| Error::Lua("jump slot poisoned".to_owned()))?;
        Ok(slot.take())
    }

    fn run_loaded_with_control(&self, program: &LuaProgram) -> Result<LuaBlockResult> {
        {
            let mut slot = self
                .jump_slot
                .lock()
                .map_err(|_| Error::Lua("jump slot poisoned".to_owned()))?;
            *slot = None;
        }
        let result = program.load(&self.lua)?.call(());
        // A recorded jump takes precedence over the chunk's error: that error
        // is the jump's own transfer marker, not a real failure. A poisoned
        // slot propagates rather than coercing into "no jump"
        // (discarded-error-001).
        if let Some(heading) = self.take_jump()? {
            return Ok(LuaBlockResult::Jump(heading));
        }
        let returned = result.map_err(|error| program.map_runtime_error(&error))?;
        Ok(LuaBlockResult::Returned(scalar_return(returned)?))
    }

    /// Starts one Lua block as a coroutine on this VM and resumes it to its
    /// first yield or its end.
    ///
    /// This is the scheduler's chunk-execution path: one coroutine per Lua
    /// block, created from the block's loaded function on this persistent
    /// VM, so the VM's globals (`var`, `reply`, the conversation state)
    /// roll forward across blocks exactly as on the legacy
    /// [`run_chunk`](Self::run_chunk) path. Instruction hooks are
    /// per-coroutine in PUC Lua, so the VM's budget/cancellation hook is
    /// installed on the fresh thread; the main-state hook from construction
    /// never fires inside a resumed coroutine.
    ///
    /// A coroutine that returns ends the block under the legacy contract: a
    /// recorded jump takes precedence over the chunk's own error, genuine
    /// failures map through [`LuaProgram::map_runtime_error`], and the
    /// scalar-return rule applies to the return values. A coroutine that
    /// yields suspends with the shim's request table; the driver resumes it
    /// with [`resume_block_coro`](Self::resume_block_coro). No observation
    /// events fire here; the driver owns the chunk observation boundaries.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the jump slot is poisoned, the program
    /// cannot load, or the thread cannot be created or hooked; a block
    /// failure returns the mapped runtime error.
    pub fn start_block_coro(&self, program: &LuaProgram) -> Result<CoroStep> {
        {
            let mut slot = self
                .jump_slot
                .lock()
                .map_err(|_| Error::Lua("jump slot poisoned".to_owned()))?;
            *slot = None;
        }
        let function = program.load(&self.lua)?;
        let thread = self.lua.create_thread(function).map_err(Error::lua)?;
        self.instruction_budget.install_on_thread(&thread)?;
        let result = thread.resume::<MultiValue>(());
        self.step_block_coro(program, thread, result)
    }

    /// Resumes a suspended block coroutine with the driver's answer values.
    ///
    /// # Errors
    /// Same contract as [`start_block_coro`](Self::start_block_coro).
    pub fn resume_block_coro(
        &self,
        program: &LuaProgram,
        thread: &Thread,
        args: impl IntoLuaMulti,
    ) -> Result<CoroStep> {
        let result = thread.resume::<MultiValue>(args);
        self.step_block_coro(program, thread.clone(), result)
    }

    /// Validates a suspended block coroutine's yielded values into the
    /// boundary outcome.
    ///
    /// A shim yields exactly its request table; anything else is
    /// [`YieldParse::Malformed`] and fails the block as a hand-rolled or
    /// corrupted yield. Author code cannot reach
    /// `coroutine.yield` (the global is stripped at shim install), so this
    /// strict validation is defense in depth. A well-formed call whose
    /// argument fails validation is [`YieldParse::Call`]: the error rides
    /// back as the answer so the shim raises it at the call site.
    #[must_use]
    pub fn request_from_yield(&self, values: &MultiValue) -> YieldParse {
        Request::from_yield(&self.lua, values.iter().next().unwrap_or(&Value::Nil))
    }

    /// Resumes a suspended block coroutine with the driver's answer.
    ///
    /// The answer renders to its `(ok, result)` envelope on this VM. On a
    /// failure answer the envelope carries only the display string for the
    /// shim to raise, and the typed error the answer owned is
    /// substituted back when the shim-raised error surfaces as the
    /// coroutine's failure (the LUA-012 contract), so the Rust caller
    /// receives the structured error rather than a string.
    ///
    /// The error type is the driver's own (`E`); this crate's internal
    /// failures convert into it through [`From`].
    ///
    /// # Errors
    /// Same contract as [`start_block_coro`](Self::start_block_coro), plus
    /// the driver's `E` if the envelope cannot be rendered on this VM.
    pub fn resume_block_coro_answer<E>(
        &self,
        program: &LuaProgram,
        thread: &Thread,
        answer: Answer<E>,
    ) -> std::result::Result<CoroStep, E>
    where
        E: std::fmt::Display + From<Error>,
    {
        let (envelope, retained) = answer.into_envelope(&self.lua).map_err(Error::lua)?;
        match self.resume_block_coro(program, thread, envelope) {
            Ok(step) => Ok(step),
            Err(error) => Err(match retained {
                Some(retained) if coroutine_failure_is(&error, &retained) => retained,
                _ => E::from(error),
            }),
        }
    }

    fn step_block_coro(
        &self,
        program: &LuaProgram,
        thread: Thread,
        result: mlua::Result<MultiValue>,
    ) -> Result<CoroStep> {
        match result {
            // A resumed thread that is still resumable suspended on a yield;
            // the yielded values are the shim's request table.
            Ok(values) if thread.status() == ThreadStatus::Resumable => {
                Ok(CoroStep::Yielded(thread, values))
            }
            result => {
                // A recorded jump takes precedence over the chunk's error,
                // exactly as on the legacy path.
                if let Some(heading) = self.take_jump()? {
                    return Ok(CoroStep::Done(LuaBlockResult::Jump(heading)));
                }
                let returned = result.map_err(|error| program.map_runtime_error(&error))?;
                Ok(CoroStep::Done(LuaBlockResult::Returned(scalar_return(
                    returned,
                )?)))
            }
        }
    }
}

/// Whether a block coroutine's failure is the shim's re-raise of the
/// answer's retained typed error: the shim raises `error(result, 0)`, so the
/// inner `mlua` runtime message's first line is exactly the retained error's
/// display. The comparison reads the retained `mlua` source rather than the
/// mapped message, whose `Display` carries mlua's `runtime error: ` prefix.
/// A block that caught the shim's error and failed on its own keeps its own
/// error.
fn coroutine_failure_is<E: std::fmt::Display>(failure: &Error, retained: &E) -> bool {
    let display = retained.to_string();
    match failure {
        Error::LuaRuntime { source, .. } => match source.downcast_ref::<mlua::Error>() {
            Some(mlua::Error::RuntimeError(message)) => {
                message.lines().next() == Some(display.as_str())
            }
            _ => false,
        },
        Error::Lua(message) => message.lines().next() == Some(display.as_str()),
        _ => false,
    }
}

/// One step of a Lua block running on a coroutine.
///
/// Produced by [`SectionVm::start_block_coro`] and
/// [`SectionVm::resume_block_coro`]; consumed by the scheduler's driver,
/// which validates a [`Yielded`](CoroStep::Yielded) request, dispatches it,
/// and resumes the thread with the answer.
#[derive(Debug)]
pub enum CoroStep {
    /// The coroutine suspended on a shim yield; the yielded values carry
    /// the request table.
    Yielded(Thread, MultiValue),
    /// The coroutine ended (return, jump, or error); the block outcome
    /// follows the legacy `run_loaded_with_control` contract.
    Done(LuaBlockResult),
}

/// The result of running a section's Lua block.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct LuaOutcome {
    /// The chunk's top-level return value, if it returned one (the finish case).
    pub(crate) returned: Option<String>,
    /// The `var` table after the block ran, as JSON, for prose substitution.
    pub(crate) var: Json,
}

/// Run a section's Lua chunk with `args` and `sys` exposed, a writable `var`
/// table available, and a `store` table backed by `store`, returning the
/// chunk's return value and the final `var`. Harness-mediated store operations
/// report safe outcomes to `observer` under `execution` and `section`.
/// `log(message)` reports constrained author checkpoints through the same
/// observer; direct `print` is unavailable.
///
/// `store` is the run-scoped virtual-file handle; every section in a run is
/// given the same handle, so files a section writes persist for later sections
/// even though each section starts a fresh context. The exposed `store` table
/// is always present (a host capability, not a scoped tool).
///
/// The `tools` table is the same validating one every section VM installs,
/// with no frozen bindings: a chunk that calls `tools.add(...)` fails loudly
/// because no alias was declared by `tools.bind`.
///
/// # Errors
/// Returns [`Error::Lua`] if the sandbox cannot be built, `sys`/`var`/`store`
/// cannot be bridged, the chunk fails to run (including hitting the instruction
/// budget or a failing `store` op, which raises a Lua error), or it returns a
/// value that cannot be rendered as a result string.
#[cfg(test)]
pub(crate) fn run_chunk(
    source: &str,
    args: &str,
    sys: &Json,
    store: &StoreRef,
    execution: &str,
    observer: &Arc<dyn Observer>,
    section: &str,
) -> Result<LuaOutcome> {
    let mut vm = SectionVm::new(&GuardNonce::fresh(), execution, observer.as_ref(), section)?;
    vm.inject_host(args, sys, store, None)?;
    vm.install_host_apis(observer, section)?;
    let returned: MultiValue = vm.lua.load(source).eval().map_err(Error::lua)?;
    let returned = scalar_return(returned)?;
    let var = vm.var()?;

    Ok(LuaOutcome { returned, var })
}

/// Reads the section's effective tool bindings without mutating the tool
/// runtime: prompt-wide `always` aliases followed by H2 `tools.add`
/// additions, each resolved against the frozen bindings with any author
/// description override applied.
///
/// Rebuilt on every prose block so `tools.add` and `tools.add_local` calls
/// between blocks reach the next model turn.
///
/// # Errors
/// Returns [`Error::Lua`] if the tool runtime's mutex is poisoned or an added
/// alias has no frozen binding.
pub fn current_tool_bindings(
    bindings: &ToolSet,
    runtime: &Mutex<ToolRuntime>,
) -> Result<Vec<ToolBinding>> {
    let runtime = runtime
        .lock()
        .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
    bindings
        .always()
        .iter()
        .chain(runtime.added.iter())
        .map(|alias| binding_for_scope(bindings, &runtime, alias))
        .collect()
}

/// Reads the section's effective model binding through the run's model view
/// without mutating the model runtime: the H2 `models.use` selection, else
/// the prompt-wide `models.default` baseline.
///
/// # Errors
/// Returns [`Error::Lua`] if the model runtime's mutex is poisoned or the
/// selected alias has no frozen binding.
pub fn resolve_model_binding(
    bindings: &dyn ModelView,
    runtime: &Mutex<ModelRuntime>,
) -> Result<Option<ModelBinding>> {
    let used = {
        let runtime = runtime
            .lock()
            .map_err(|_| Error::Lua("model declaration runtime was poisoned".to_owned()))?;
        runtime.used().map(String::from)
    };
    let alias = match used {
        Some(alias) => Some(alias),
        None => bindings.default()?,
    };
    match alias {
        Some(alias) => Ok(Some(bindings.binding(&alias)?.ok_or_else(|| {
            Error::Lua(format!("model alias {alias:?} has no frozen binding"))
        })?)),
        None => Ok(None),
    }
}

/// Clones a frozen binding and applies any author model-description override.
pub(crate) fn binding_for_scope(
    bindings: &ToolSet,
    runtime: &ToolRuntime,
    alias: &str,
) -> Result<ToolBinding> {
    let mut binding = bindings
        .binding(alias)
        .cloned()
        .ok_or_else(|| Error::Lua(format!("tool alias {alias:?} has no frozen binding")))?;
    if let Some(description) = runtime.description_overrides.get(alias) {
        binding.model_description = Some(description.clone());
    }
    Ok(binding)
}
