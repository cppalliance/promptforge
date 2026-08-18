#[cfg(test)]
use super::install_store_table_scoped;
use super::{
    Arc, AtomicU32, AtomicUsize, BTreeMap, DEFAULT_LUA_LOG_EVENTS, DEFAULT_LUA_MEMORY_BYTES, Error,
    Function, Json, Lua, LuaBlockResult, LuaFanoutResult, LuaModelHandle, LuaOptions, LuaProgram,
    LuaSectionHandle, LuaSerdeExt, LuaToolHandle, ModelBinding, ModelBindings, ModelInferHook,
    ModelRuntime, MultiValue, Mutex, Observer, Ordering, Result, RuntimeResolution, StdLib,
    StoreRef, ToolBinding, ToolBindings, ToolCallCounts, ToolRuntime, Value,
    default_log_byte_budget, detail, harden, install_h2_models, install_h2_tools,
    install_instruction_budget, install_log, install_log_scoped, install_lua_tool_calls,
    install_store_table, install_tasks_table, resolve_section_target, scalar_return, seal_sys,
};
use crate::client::ToolSchema;

/// One hardened, isolated Lua VM for a section's complete lifecycle.
///
/// The VM owns one Lua environment from construction until drop. An optional
/// shared program runs before host values are installed, then Lua chunks
/// loaded with [`run_chunk`](Self::run_chunk) see that same environment.
/// [`bind_reply`](Self::bind_reply) inserts the model reply into it between
/// chunks. A single instruction counter covers every program run by this
/// VM, so splitting work across chunks cannot reset the budget.
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
/// use promptforge_core::lua::SectionVm;
/// use promptforge_core::observe::NullObserver;
///
/// let vm = SectionVm::new(None, "example-run", &NullObserver::default(), "Example")?;
/// vm.teardown(&NullObserver::default(), "Example");
/// # Ok::<(), promptforge_core::Error>(())
/// ```
#[derive(Debug)]
pub(crate) struct SectionVm {
    execution: String,
    lua: Lua,
    bound_tools: ToolBindings,
    bound_models: ModelBindings,
    pub(crate) tool_runtime: Arc<Mutex<ToolRuntime>>,
    pub(crate) model_runtime: Arc<Mutex<ModelRuntime>>,
    /// Shared with [`crate::execute::InferContext`] so `model:infer` and the
    /// prose tool loop increment the same `tools.calls` counters.
    counts_slot: Arc<Mutex<Option<ToolCallCounts>>>,
    /// Set by Lua `jump` before it aborts the current chunk.
    jump_slot: Arc<Mutex<Option<String>>>,
    /// Live sealed `sys` JSON shared with `model:infer` for finish-reason updates.
    sys_live: Arc<Mutex<Option<Json>>>,
    store: Option<StoreRef>,
    host_injected: bool,
    /// Remaining `log()` events this VM may emit before the budget is exhausted.
    log_budget: Arc<AtomicU32>,
    /// Remaining cumulative `log()` message bytes this VM may emit. Bounds total
    /// log volume even when each event is under the per-event ceilings.
    log_byte_budget: Arc<AtomicUsize>,
    /// Local tools registered by Lua code, dispatched back into this VM.
    local_tools: LocalTools,
}

/// Local tool registrations owned by a section VM.
///
/// Each entry holds the tool alias, its prebuilt schema, and the registry key
/// for the Lua handler function captured at registration time. The entries are
/// shared with the `tools.local` Lua callback, which must be `Send`, hence the
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
    #[must_use]
    pub(crate) fn schemas(&self) -> Vec<ToolSchema> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(_, schema, _)| schema.clone())
            .collect()
    }

    /// Returns whether `alias` names a registered local tool.
    #[must_use]
    pub(crate) fn contains(&self, alias: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .any(|(name, _, _)| name == alias)
    }

    /// Calls the handler registered under `alias` with JSON `args`.
    ///
    /// The `jump` global is nilled for the handler's duration and restored
    /// afterward: a local tool runs outside any chunk's control flow, so a
    /// jump recorded here would surface stale at the next chunk boundary.
    /// Handlers may still call `execute()`, `fanout`, and `model:infer`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if no local tool is registered under `alias`,
    /// the args cannot be bridged, the jump guard cannot be applied or
    /// restored, the handler fails, or it returns a non-scalar value.
    pub(crate) fn call(&self, lua: &Lua, alias: &str, args: Json) -> Result<String> {
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
        let table = lua.to_value(&args).map_err(Error::lua)?;
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
    /// Creates a hardened section VM and optionally executes a shared program.
    ///
    /// The shared program runs before `args`, `sys`, `var`, `tools`, `store`,
    /// and `reply` are installed. This delayed injection prevents shared code
    /// from retaining a host value before section execution begins. The VM
    /// retains `execution` for every later lifecycle report. Shared execution
    /// receives a phase-local `log(message)` callback; direct `print` is
    /// unavailable.
    ///
    /// The VM carries no frozen tool bindings, so the validating `tools.add`
    /// installed by [`inject_host`](Self::inject_host) rejects every alias as
    /// undeclared: a prompt without `tools.need` declarations cannot scope
    /// tools.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the VM cannot be built or hardened, or if the
    /// shared program fails or returns a non-scalar value.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    ///
    /// let shared = LuaProgram::compile(
    ///     "function decorate(s) return '<' .. s .. '>' end",
    ///     "shared",
    ///     1,
    ///     "example-run",
    ///     &NullObserver::default(),
    ///     "Example",
    /// )?;
    /// let vm = SectionVm::new(Some(&shared), "example-run", &NullObserver::default(), "Example")?;
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn new(
        shared: Option<&LuaProgram>,
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
        let vm = Self {
            execution: execution.to_owned(),
            lua,
            bound_tools: ToolBindings::default(),
            bound_models: <ModelBindings as Default>::default(),
            tool_runtime: Arc::new(Mutex::new(ToolRuntime {
                added: Vec::new(),
                description_overrides: BTreeMap::new(),
                generation: 0,
            })),
            model_runtime: Arc::new(Mutex::new(ModelRuntime::new())),
            counts_slot: Arc::new(Mutex::new(None)),
            jump_slot: Arc::new(Mutex::new(None)),
            sys_live: Arc::new(Mutex::new(None)),
            store: None,
            host_injected: false,
            log_budget: Arc::new(AtomicU32::new(DEFAULT_LUA_LOG_EVENTS)),
            log_byte_budget: Arc::new(AtomicUsize::new(default_log_byte_budget(
                DEFAULT_LUA_LOG_EVENTS,
            ))),
            local_tools: LocalTools::default(),
        };
        if let Err(error) = harden(&vm.lua) {
            return vm.construction_failed(error, observer, section);
        }
        install_instruction_budget(&vm.lua);
        if let Some(program) = shared {
            observer.observe(execution, section, detail::LUA_SHARED_LOAD_STARTED);
            match vm.run_loaded_with_log(program, observer, section) {
                Ok(_) => observer.observe(execution, section, detail::LUA_SHARED_LOAD_SUCCEEDED),
                Err(error) => {
                    observer.observe(execution, section, detail::LUA_SHARED_LOAD_FAILED);
                    return vm.construction_failed(error, observer, section);
                }
            }
        }
        Ok(vm)
    }

    /// Creates a section VM, loads its shared library, then installs captured bindings.
    ///
    /// The shared program runs before any host API, including `log`, or captured
    /// binding exists.
    /// Its functions may refer to those globals because Lua resolves globals
    /// when a function is called. Rust installs each captured Tool and Model
    /// object directly after the shared load, without replaying H1 code.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if VM construction, shared-library execution, or
    /// captured-binding installation fails.
    pub(crate) fn new_for_section(
        replay: Option<&LuaProgram>,
        tools: &ToolBindings,
        models: &ModelBindings,
        execution: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Self> {
        let mut vm = Self::new(None, execution, observer, section)?;
        if let Some(program) = replay {
            observer.observe(execution, section, detail::LUA_SHARED_LOAD_STARTED);
            match vm.run_loaded_without_host(program) {
                Ok(_) => observer.observe(execution, section, detail::LUA_SHARED_LOAD_SUCCEEDED),
                Err(error) => {
                    observer.observe(execution, section, detail::LUA_SHARED_LOAD_FAILED);
                    return vm.construction_failed(error, observer, section);
                }
            }
        }
        vm.bound_tools = tools.clone();
        vm.bound_models = models.clone();
        if let Err(error) = vm.install_captured_bindings() {
            return vm.construction_failed(error, observer, section);
        }
        Ok(vm)
    }

    fn install_captured_bindings(&self) -> Result<()> {
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
            let userdata = self
                .lua
                .create_userdata(LuaModelHandle::from_binding(binding))
                .map_err(Error::lua)?;
            globals
                .raw_set(binding.alias(), userdata)
                .map_err(Error::lua)?;
        }
        Ok(())
    }

    /// Installs the section's host values after the shared program has run.
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
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("input", &serde_json::json!({ "id": 1 }), &StoreRef::memory(), None)?;
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn inject_host(
        &mut self,
        args: &str,
        sys: &Json,
        store: &StoreRef,
        last_reply: Option<&str>,
    ) -> Result<()> {
        self.inject_host_with_var(args, sys, store, last_reply, None)
    }

    /// Installs host values while seeding `var` from an earlier VM.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values cannot be bridged or were already
    /// injected.
    pub(crate) fn inject_host_with_var(
        &mut self,
        args: &str,
        sys: &Json,
        store: &StoreRef,
        last_reply: Option<&str>,
        initial_var: Option<&Json>,
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
        let var = match initial_var {
            Some(value) => self.lua.to_value(value).map_err(Error::lua)?,
            None => Value::Table(self.lua.create_table().map_err(Error::lua)?),
        };
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
    pub(crate) fn install_host_apis(
        &self,
        observer: &Arc<dyn Observer>,
        section: &str,
    ) -> Result<()> {
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
        )
    }

    /// Installs `tasks`, `execute`, `jump`, and `fanout` as persistent
    /// globals for the section's whole lifecycle.
    ///
    /// Called once by the engine after host injection. Both callbacks own
    /// their run context, so the closures stay valid across every chunk this
    /// VM runs without a live [`mlua::Scope`]. The `jump` closure captures a
    /// clone of the VM's jump slot; the slot is reset before each chunk and
    /// read after it by the control-run path.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if any global cannot be installed.
    pub(crate) fn install_control_globals<E, F>(
        &self,
        tasks: &[LuaSectionHandle],
        execute_callback: E,
        fanout_callback: F,
    ) -> Result<()>
    where
        E: Fn(Value, Option<String>) -> std::result::Result<String, Error> + Send + 'static,
        F: Fn(String, String) -> std::result::Result<Vec<LuaFanoutResult>, Error> + Send + 'static,
    {
        install_tasks_table(&self.lua, tasks)?;
        let globals = self.lua.globals();
        let execute_fn = self
            .lua
            .create_function(move |_, (target, input): (Value, Option<String>)| {
                execute_callback(target, input).map_err(mlua::Error::external)
            })
            .map_err(Error::lua)?;
        globals.raw_set("execute", execute_fn).map_err(Error::lua)?;
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
        globals.raw_set("jump", jump_fn).map_err(Error::lua)?;
        let fanout_fn = self
            .lua
            .create_function(move |lua, (worker, list): (String, String)| {
                let replies = fanout_callback(worker, list).map_err(mlua::Error::external)?;
                let table = lua.create_table_with_capacity(replies.len(), 0)?;
                for (i, reply) in replies.into_iter().enumerate() {
                    table.raw_set(i + 1, reply)?;
                }
                Ok(table)
            })
            .map_err(Error::lua)?;
        globals.raw_set("fanout", fanout_fn).map_err(Error::lua)
    }

    /// Executes one live H1 Lua block with call-time capability resolution.
    ///
    /// Resolver callbacks are scoped to this block and reinstalled for each
    /// later H1 Lua block. Resolved Tool and Model objects remain ordinary Lua
    /// values in the VM.
    ///
    /// # Errors
    /// Returns typed capability errors captured by the runtime resolver, or the
    /// underlying Lua execution error.
    pub(crate) fn run_live_h1_block(
        &self,
        program: &LuaProgram,
        resolution: &RuntimeResolution<'_, '_>,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        let result = self.lua.scope(|scope| {
            resolution
                .install(&self.lua, scope)
                .map_err(mlua::Error::external)?;
            self.run_chunk(program, observer, section)
                .map_err(mlua::Error::external)
        });
        match result {
            Ok(LuaBlockResult::Returned(value)) => Ok(value),
            // Control globals are never installed on the H1 VM, so `jump` is
            // nil there; this arm is defensive against a recorded jump.
            Ok(LuaBlockResult::Jump(heading)) => Err(Error::Lua(format!(
                "jump({heading}) is not available in live H1 Lua"
            ))),
            Err(error) => match resolution.take_callback_error()? {
                Some(error) => Err(error),
                None => Err(Error::lua(error)),
            },
        }
    }

    /// Replaces the sealed Lua `sys` global after scope close.
    ///
    /// Host injection must have run first. Used to expose `sys.model` once the
    /// section's model binding is fixed.
    pub(crate) fn re_seal_sys(&self, sys: &Json) -> Result<()> {
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

    /// Shared live `sys` JSON for `model:infer` finish-reason updates.
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
    pub(crate) fn current_sys(&self, fallback: &Json) -> Result<Json> {
        let guard = self
            .sys_live
            .lock()
            .map_err(|_| Error::Lua("sys live slot was poisoned".to_owned()))?;
        Ok(guard.clone().unwrap_or_else(|| fallback.clone()))
    }

    /// Executes a compiled Lua chunk in this VM's persistent environment.
    ///
    /// This is the one path for running a section's Lua blocks. StoreRef and
    /// `log` reports go to the observer captured by
    /// [`install_host_apis`](Self::install_host_apis); a nil or absent
    /// top-level return produces [`LuaBlockResult::Returned`]`(None)`. When
    /// the chunk may call `tasks`, `execute`, `jump`, or `fanout`, those must
    /// already be installed by
    /// [`install_control_globals`](Self::install_control_globals).
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, execution
    /// fails, the shared instruction budget is exhausted, or the program
    /// returns a non-scalar value.
    pub(crate) fn run_chunk(
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
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// vm.bind_reply("model answer", &NullObserver::default(), "Example")?;
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn bind_reply(
        &self,
        reply: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<()> {
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

    /// Returns the current `var` table as JSON.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected or `var`
    /// cannot be represented as JSON.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// assert_eq!(vm.var()?, serde_json::json!({}));
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn var(&self) -> Result<Json> {
        if !self.host_injected {
            return Err(Error::Lua(
                "section VM host values have not been injected".to_owned(),
            ));
        }
        let value: Value = self.lua.globals().get("var").map_err(Error::lua)?;
        self.lua.from_value(value).map_err(Error::lua)
    }

    /// Sets a string global in the VM, overwriting any existing value.
    ///
    /// Used by fanout to inject `item` after host injection.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the global cannot be set.
    pub(crate) fn set_global_string(&self, name: &str, value: &str) -> Result<()> {
        self.lua.globals().raw_set(name, value).map_err(Error::lua)
    }

    /// Installs `tools.calls` as a read-only Lua table backed by the shared
    /// [`ToolCallCounts`]. Each in-scope alias reads its live count; indexing
    /// an unknown key is a hard error that names the bad key and lists the
    /// in-scope set. When the key was declared by `tools.need` but not added
    /// to this section's scope, the diagnostic says so.
    ///
    /// Returns the `ToolCallCounts` handle so the executor's tool loop can
    /// increment it. Reuses counts already seeded by `model:infer` so counters
    /// persist across infer and the prose path.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when installing the `tools.calls` index fails.
    pub(crate) fn install_tool_call_counts(
        &self,
        bindings: &[ToolBinding],
    ) -> Result<ToolCallCounts> {
        let counts = {
            let mut slot = self
                .counts_slot
                .lock()
                .map_err(|_| Error::Lua("tool call counts mutex was poisoned".to_owned()))?;
            if let Some(existing) = slot.as_ref() {
                for binding in bindings {
                    existing.ensure(binding.alias())?;
                }
                existing.clone()
            } else {
                let created =
                    ToolCallCounts::new(bindings.iter().map(|b| b.alias().to_owned()));
                *slot = Some(created.clone());
                created
            }
        };
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
    #[must_use]
    pub(crate) fn tool_bag_handles(&self) -> (ToolBindings, Arc<Mutex<ToolRuntime>>) {
        (self.bound_tools.clone(), Arc::clone(&self.tool_runtime))
    }

    /// Returns frozen model bindings and the live H2 selection runtime.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn model_bag_handles(&self) -> (ModelBindings, Arc<Mutex<ModelRuntime>>) {
        (self.bound_models.clone(), Arc::clone(&self.model_runtime))
    }

    /// Returns the shared tool-call counts slot for `model:infer`.
    #[must_use]
    pub(crate) fn counts_slot(&self) -> Arc<Mutex<Option<ToolCallCounts>>> {
        Arc::clone(&self.counts_slot)
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
    pub(crate) fn call_local_tool(&self, alias: &str, args: Json) -> Result<String> {
        self.local_tools.call(&self.lua, alias, args)
    }

    /// Returns the schemas of every registered local tool.
    #[must_use]
    pub(crate) fn local_tool_schemas(&self) -> Vec<ToolSchema> {
        self.local_tools.schemas()
    }

    /// Returns the shared local-tools registry for the `model:infer` tool bag.
    #[must_use]
    pub(crate) fn local_tools_handle(&self) -> LocalTools {
        self.local_tools.clone()
    }

    /// Returns whether `alias` names a registered local tool.
    #[must_use]
    #[allow(dead_code)] // wired up by the local-tools dispatch step
    pub(crate) fn has_local_tool(&self, alias: &str) -> bool {
        self.local_tools.contains(alias)
    }

    /// Applies the run's Lua resource limits to this VM.
    ///
    /// Sets the heap ceiling (`lua_memory_bytes`) and resets the `log()` event
    /// budget (`lua_log_events`). Called by the executor right after
    /// construction so a VM honors the caller's [`RunLimits`] rather than only
    /// the safe non-env defaults installed in [`SectionVm::new`].
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the underlying VM rejects the memory limit.
    pub(crate) fn apply_lua_limits(&self, memory_bytes: usize, log_events: u32) -> Result<()> {
        self.lua
            .set_memory_limit(memory_bytes)
            .map_err(Error::lua)?;
        self.log_budget.store(log_events, Ordering::Relaxed);
        self.log_byte_budget
            .store(default_log_byte_budget(log_events), Ordering::Relaxed);
        Ok(())
    }

    /// Installs the `model:infer` host hook for this VM's Lua state.
    pub(crate) fn set_infer_hook(&self, hook: ModelInferHook) {
        self.lua.set_app_data(hook);
    }

    /// Clears the `model:infer` host hook.
    pub(crate) fn clear_infer_hook(&self) {
        let _ = self.lua.remove_app_data::<ModelInferHook>();
    }

    /// Destroys this section VM at an explicit observed lifecycle boundary.
    ///
    /// The observer is borrowed only for this synchronous call and is not
    /// retained by the VM.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    ///
    /// let vm = SectionVm::new(None, "example-run", &NullObserver::default(), "Example")?;
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn teardown(self, observer: &dyn Observer, section: &str) {
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

    fn run_loaded_with_log(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        let returned: MultiValue = self
            .lua
            .scope(|scope| {
                install_log_scoped(
                    &self.lua,
                    scope,
                    &self.execution,
                    observer,
                    section,
                    &self.log_budget,
                    &self.log_byte_budget,
                )
                .map_err(mlua::Error::external)?;
                let result = program
                    .load(&self.lua)
                    .map_err(mlua::Error::external)?
                    .call(());
                // The phase-local scoped callback dies with the scope; clear
                // the global so no dangling reference survives into the
                // host-injected phase.
                let cleanup = self.lua.globals().raw_set("log", Value::Nil);
                match (result, cleanup) {
                    (Err(error), _) | (Ok(_), Err(error)) => Err(error),
                    (Ok(value), Ok(())) => Ok(value),
                }
            })
            .map_err(|error| program.map_runtime_error(&error))?;
        scalar_return(returned)
    }

    fn run_loaded_without_host(&self, program: &LuaProgram) -> Result<Option<String>> {
        let returned: MultiValue = program
            .load(&self.lua)?
            .call(())
            .map_err(|error| program.map_runtime_error(&error))?;
        scalar_return(returned)
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
            .map_err(|_| Error::Lua("jump slot was poisoned".to_owned()))?;
        Ok(slot.take())
    }

    fn run_loaded_with_control(&self, program: &LuaProgram) -> Result<LuaBlockResult> {
        {
            let mut slot = self
                .jump_slot
                .lock()
                .map_err(|_| Error::Lua("jump slot was poisoned".to_owned()))?;
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

    #[cfg(test)]
    fn run_source(
        &self,
        source: &str,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        let store = self.store.as_ref().ok_or_else(|| {
            Error::Lua("section VM host values have not been injected".to_owned())
        })?;
        let returned: MultiValue = self
            .lua
            .scope(|scope| {
                install_log_scoped(
                    &self.lua,
                    scope,
                    &self.execution,
                    observer,
                    section,
                    &self.log_budget,
                    &self.log_byte_budget,
                )
                .map_err(mlua::Error::external)?;
                install_store_table_scoped(
                    &self.lua,
                    scope,
                    &self.lua.globals(),
                    store,
                    &self.execution,
                    observer,
                    section,
                )
                .map_err(mlua::Error::external)?;
                let result = self.lua.load(source).eval();
                let cleanup = self.lua.globals().raw_set("log", Value::Nil);
                match (result, cleanup) {
                    (Err(error), _) | (Ok(_), Err(error)) => Err(error),
                    (Ok(value), Ok(())) => Ok(value),
                }
            })
            .map_err(Error::lua)?;
        scalar_return(returned)
    }
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
/// observer for this call only; direct `print` is unavailable.
///
/// `store` is the run-scoped virtual-file handle; every section in a run is
/// given the same handle, so files a section writes persist for later sections
/// even though each section starts a fresh context. The exposed `store` table
/// is always present (a host capability, not a scoped tool).
///
/// The `tools` table is the same validating one every section VM installs,
/// with no frozen bindings: a chunk that calls `tools.add(...)` fails loudly
/// because no alias was declared by `tools.need`.
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
    observer: &dyn Observer,
    section: &str,
) -> Result<LuaOutcome> {
    let mut vm = SectionVm::new(None, execution, observer, section)?;
    vm.inject_host(args, sys, store, None)?;
    let returned = vm.run_source(source, observer, section)?;
    let var = vm.var()?;

    Ok(LuaOutcome { returned, var })
}

/// Reads the section's effective tool bindings without mutating the tool
/// runtime: prompt-wide `always` aliases followed by H2 `tools.add`
/// additions, each resolved against the frozen bindings with any author
/// description override applied.
///
/// Rebuilt on every prose block so `tools.add` and `tools.local` calls
/// between blocks reach the next model turn.
pub(crate) fn current_tool_bindings(
    bindings: &ToolBindings,
    runtime: &Mutex<ToolRuntime>,
) -> Result<Vec<ToolBinding>> {
    let runtime = runtime
        .lock()
        .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
    bindings
        .always
        .iter()
        .chain(runtime.added.iter())
        .map(|alias| binding_for_scope(bindings, &runtime, alias))
        .collect()
}

/// Reads the section's effective model binding without mutating the model
/// runtime: the H2 `models.use` selection, else the prompt-wide
/// `models.default` baseline.
pub(crate) fn resolve_model_binding(
    bindings: &ModelBindings,
    runtime: &Mutex<ModelRuntime>,
) -> Result<Option<ModelBinding>> {
    let alias = {
        let runtime = runtime
            .lock()
            .map_err(|_| Error::Lua("model declaration runtime was poisoned".to_owned()))?;
        runtime
            .used()
            .map(String::from)
            .or_else(|| bindings.default().map(String::from))
    };
    match alias {
        Some(alias) => Ok(Some(bindings.binding(&alias).cloned().ok_or_else(
            || Error::Lua(format!("model alias {alias:?} has no frozen binding")),
        )?)),
        None => Ok(None),
    }
}

/// Clones a frozen binding and applies any author model-description override.
pub(crate) fn binding_for_scope(
    bindings: &ToolBindings,
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
