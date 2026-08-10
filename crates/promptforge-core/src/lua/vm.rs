use super::{
    Arc, AtomicU32, AtomicUsize, BTreeMap, ClosedScopes, DEFAULT_LUA_LOG_EVENTS,
    DEFAULT_LUA_MEMORY_BYTES, Error, Json, Lua, LuaBlockResult, LuaFanoutResult, LuaModelHandle,
    LuaOptions, LuaProgram, LuaSectionHandle, LuaSerdeExt, LuaToolHandle, ModelBindings,
    ModelInferHook, ModelRuntime, MultiValue, Mutex, Observer, Ordering, Result, RuntimeResolution,
    StdLib, StoreRef, ToolBinding, ToolBindings, ToolCallCounts, ToolPhase, ToolRuntime, ToolScope,
    Value, close_model_scope, default_log_byte_budget, detail, finish_log_phase, harden,
    install_h2_models, install_h2_tools, install_instruction_budget, install_log,
    install_lua_tool_calls, install_store_table, install_tasks_table, resolve_section_target,
    scalar_return, seal_sys,
};

/// One hardened, isolated Lua VM for a section's complete lifecycle.
///
/// The VM owns one Lua environment from construction until drop. An optional
/// shared program runs before host values are installed, then prologue and
/// epilog programs loaded with [`run_prologue`](Self::run_prologue) and
/// [`run_epilog`](Self::run_epilog) see that same environment.
/// [`bind_reply`](Self::bind_reply) inserts the model reply into it between
/// those phases. A single instruction counter covers every program run by this
/// VM, so splitting work across lifecycle phases cannot reset the budget.
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
            bound_models: ModelBindings::default(),
            tool_runtime: Arc::new(Mutex::new(ToolRuntime {
                phase: ToolPhase::H2,
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
    /// clone of the run-scoped store. StoreRef functions are installed with
    /// phase-local borrowed observation context by
    /// [`run_prologue`](Self::run_prologue) and [`run_epilog`](Self::run_epilog),
    /// so no observer reference is retained while the VM waits for a model reply.
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
        install_h2_tools(&self.lua, &globals, &self.bound_tools, &self.tool_runtime)?;
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
            self.run_prologue(program, observer, section)
                .map_err(mlua::Error::external)
        });
        match result {
            Ok(value) => Ok(value),
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

    /// Executes a compiled prologue in this VM's persistent environment.
    ///
    /// StoreRef-operation reports recorded by host callbacks are delivered in
    /// operation order before this method returns, including when execution
    /// fails. A nil or absent top-level return produces `None`; strings,
    /// integers, numbers, and booleans produce their scalar string form.
    /// `log(message)` is available only for this call and reports under
    /// `execution` and `section`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, execution
    /// fails, the shared instruction budget is exhausted, or the program
    /// returns a non-scalar value.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let prologue = LuaProgram::compile(
    ///     "var.answer = 42",
    ///     "prologue",
    ///     1,
    ///     "example-run",
    ///     &NullObserver::default(),
    ///     "Example",
    /// )?;
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// assert_eq!(vm.run_prologue(&prologue, &NullObserver::default(), "Example")?, None);
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn run_prologue(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        observer.observe(&self.execution, section, detail::LUA_PROLOGUE_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_PROLOGUE_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_host(program, observer, section);
        observer.observe(
            &self.execution,
            section,
            if result.is_ok() {
                detail::LUA_PROLOGUE_SUCCEEDED
            } else {
                detail::LUA_PROLOGUE_FAILED
            },
        );
        result
    }

    /// Executes a compiled prologue with `tasks`, `execute`, `jump`, and optional `fanout`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected or
    /// execution fails.
    pub(crate) fn run_prologue_with_control<E, F>(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
        tasks: &[LuaSectionHandle],
        execute_callback: Option<&E>,
        fanout_callback: Option<&F>,
    ) -> Result<LuaBlockResult>
    where
        E: Fn(Value, Option<String>) -> std::result::Result<String, Error>,
        F: Fn(String, String) -> std::result::Result<Vec<LuaFanoutResult>, Error>,
    {
        observer.observe(&self.execution, section, detail::LUA_PROLOGUE_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_PROLOGUE_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_control(
            program,
            observer,
            section,
            tasks,
            execute_callback,
            fanout_callback,
            true,
        );
        let ok = result.is_ok();
        observer.observe(
            &self.execution,
            section,
            if ok {
                detail::LUA_PROLOGUE_SUCCEEDED
            } else {
                detail::LUA_PROLOGUE_FAILED
            },
        );
        result
    }

    /// Binds the model reply for a later epilog in the same environment.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, the tool
    /// scope remains open, or the reply cannot be installed.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::SectionVm;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// vm.close_tool_scope(&NullObserver::default(), "Example")?;
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
        if let Err(error) = self.require_closed_tool_scope("bind a model reply") {
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

    /// Executes a compiled epilog in this VM's persistent environment.
    ///
    /// StoreRef-operation reports are delivered in operation order between the
    /// epilog's start and outcome reports. `log(message)` is available only for
    /// this call and reports under `execution` and `section`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, the tool
    /// scope remains open, execution fails, the shared instruction budget is
    /// exhausted, or the program returns a non-scalar value.
    ///
    /// # Examples
    /// ```text
    /// use promptforge_core::lua::{LuaProgram, SectionVm};
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    ///
    /// let epilog = LuaProgram::compile(
    ///     "return reply",
    ///     "epilog",
    ///     1,
    ///     "example-run",
    ///     &NullObserver::default(),
    ///     "Example",
    /// )?;
    /// let mut vm = SectionVm::new(None, "example-run", &NullObserver::default(), "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// vm.close_tool_scope(&NullObserver::default(), "Example")?;
    /// vm.bind_reply("done", &NullObserver::default(), "Example")?;
    /// assert_eq!(
    ///     vm.run_epilog(&epilog, &NullObserver::default(), "Example")?.as_deref(),
    ///     Some("done"),
    /// );
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    pub(crate) fn run_epilog(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        observer.observe(&self.execution, section, detail::LUA_EPILOG_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        if let Err(error) = self.require_closed_tool_scope("run an epilog") {
            observer.observe(&self.execution, section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_host(program, observer, section);
        observer.observe(
            &self.execution,
            section,
            if result.is_ok() {
                detail::LUA_EPILOG_SUCCEEDED
            } else {
                detail::LUA_EPILOG_FAILED
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

    /// Executes a compiled epilog with `tasks`, `execute`, `jump`, and optional `fanout`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if host values have not been injected, the tool
    /// scope is open, or execution fails.
    pub(crate) fn run_epilog_with_control<E, F>(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
        tasks: &[LuaSectionHandle],
        execute_callback: Option<&E>,
        fanout_callback: Option<&F>,
    ) -> Result<LuaBlockResult>
    where
        E: Fn(Value, Option<String>) -> std::result::Result<String, Error>,
        F: Fn(String, String) -> std::result::Result<Vec<LuaFanoutResult>, Error>,
    {
        observer.observe(&self.execution, section, detail::LUA_EPILOG_STARTED);
        if !self.host_injected {
            let error = Error::Lua("section VM host values have not been injected".to_owned());
            observer.observe(&self.execution, section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        if let Err(error) = self.require_closed_tool_scope("run an epilog") {
            observer.observe(&self.execution, section, detail::LUA_EPILOG_FAILED);
            return Err(error);
        }
        let result = self.run_loaded_with_control(
            program,
            observer,
            section,
            tasks,
            execute_callback,
            fanout_callback,
            true,
        );
        let ok = result.is_ok();
        observer.observe(
            &self.execution,
            section,
            if ok {
                detail::LUA_EPILOG_SUCCEEDED
            } else {
                detail::LUA_EPILOG_FAILED
            },
        );
        result
    }

    /// Closes and returns this section's effective tool scope.
    ///
    /// Prompt-wide `tools.always` aliases come first, followed by first-seen
    /// `tools.add` aliases from the H2 prologue. Closing is one-way: retained
    /// function references cannot add tools during an epilog.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] for a poisoned declaration runtime, a closure
    /// attempt before host injection, or a second closure attempt.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::lua::{SectionVm, ToolBindings};
    /// use promptforge_core::model::ModelBindings;
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::store::StoreRef;
    /// let mut vm = SectionVm::new_for_section(
    ///     None,
    ///     &ToolBindings::default(),
    ///     &ModelBindings::default(),
    ///     "example-run",
    ///     &NullObserver::default(),
    ///     "Example",
    /// )?;
    /// vm.inject_host("", &serde_json::json!({}), &StoreRef::memory(), None)?;
    /// let scope = vm.close_tool_scope(&NullObserver::default(), "Example")?;
    /// assert!(scope.bindings().is_empty());
    /// vm.teardown(&NullObserver::default(), "Example");
    /// # Ok::<(), promptforge_core::Error>(())
    /// ```
    /// Closes and returns this section's effective tool scope.
    ///
    /// Also closes model selection recording. Prefer [`Self::close_scopes`] when
    /// the caller needs the section's `models.use` selection.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] for a poisoned declaration runtime, a closure
    /// attempt before host injection, or a second closure attempt.
    #[cfg(test)]
    pub(crate) fn close_tool_scope(
        &self,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<ToolScope> {
        Ok(self.close_scopes(observer, section)?.tools)
    }

    /// Closes tool and model H2 recording for this section.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] for a poisoned declaration runtime, a closure
    /// attempt before host injection, or a second closure attempt.
    pub(crate) fn close_scopes(
        &self,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<ClosedScopes> {
        observer.observe(&self.execution, section, detail::TOOL_SCOPE_CLOSING);
        // LUA-008: validate and compute BOTH scopes before committing either, so
        // a model-close failure cannot leave the tool scope already committed to
        // Closed. Phase 1 validates the tool phase and computes the effective
        // tool scope WITHOUT committing; the model close then validates+resolves;
        // only after both succeed is the (infallible) tool commit performed.
        let tools = self.prepare_tool_scope();
        observer.observe(
            &self.execution,
            section,
            if tools.is_ok() {
                detail::TOOL_SCOPE_CLOSED
            } else {
                detail::TOOL_SCOPE_FAILED
            },
        );
        let tools = tools?;
        let model = close_model_scope(
            &self.bound_models,
            &self.model_runtime,
            &self.execution,
            observer,
            section,
        )?;
        // Both scopes validated (and the model phase committed); the tool commit
        // below only flips an enum and cannot fail.
        self.commit_tool_scope_closed()?;
        Ok(ClosedScopes { tools, model })
    }

    /// Validates the tool phase is open and computes the effective tool scope
    /// WITHOUT committing the phase transition (see [`Self::close_scopes`]).
    fn prepare_tool_scope(&self) -> Result<ToolScope> {
        let bindings = &self.bound_tools;
        let runtime = self
            .tool_runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if runtime.phase != ToolPhase::H2 {
            return Err(Error::Lua(
                "tool scope can only close once after H2 recording".to_owned(),
            ));
        }
        let aliases = bindings
            .always
            .iter()
            .chain(runtime.added.iter())
            .cloned()
            .collect::<Vec<_>>();
        let effective = aliases
            .iter()
            .map(|alias| binding_for_scope(bindings, &runtime, alias))
            .collect::<Result<Vec<_>>>()?;
        Ok(ToolScope {
            bindings: effective,
        })
    }

    /// Commits the tool scope's H2 -> Closed transition. Infallible apart from a
    /// poisoned lock; only transitions from `H2` so a double call is safe.
    fn commit_tool_scope_closed(&self) -> Result<()> {
        let mut runtime = self
            .tool_runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if runtime.phase == ToolPhase::H2 {
            runtime.phase = ToolPhase::Closed;
        }
        Ok(())
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
    pub(crate) fn install_tool_call_counts(&mut self, scope: &ToolScope) -> Result<ToolCallCounts> {
        let counts = {
            let mut slot = self
                .counts_slot
                .lock()
                .map_err(|_| Error::Lua("tool call counts mutex was poisoned".to_owned()))?;
            if let Some(existing) = slot.as_ref() {
                for binding in scope.bindings() {
                    existing.ensure(binding.alias())?;
                }
                existing.clone()
            } else {
                let created =
                    ToolCallCounts::new(scope.bindings().iter().map(|b| b.alias().to_owned()));
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

    /// Returns the shared tool-call counts slot for `model:infer`.
    #[must_use]
    pub(crate) fn counts_slot(&self) -> Arc<Mutex<Option<ToolCallCounts>>> {
        Arc::clone(&self.counts_slot)
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

    fn require_closed_tool_scope(&self, operation: &str) -> Result<()> {
        let runtime = self
            .tool_runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if runtime.phase == ToolPhase::Closed {
            Ok(())
        } else {
            Err(Error::Lua(format!(
                "tool scope must close before the section VM can {operation}"
            )))
        }
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
                install_log(
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
                finish_log_phase(&self.lua, result)
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

    fn run_loaded_with_host(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
    ) -> Result<Option<String>> {
        let store = self.store.as_ref().ok_or_else(|| {
            Error::Lua("section VM host values have not been injected".to_owned())
        })?;
        let returned: MultiValue = self
            .lua
            .scope(|scope| {
                install_log(
                    &self.lua,
                    scope,
                    &self.execution,
                    observer,
                    section,
                    &self.log_budget,
                    &self.log_byte_budget,
                )
                .map_err(mlua::Error::external)?;
                install_store_table(
                    &self.lua,
                    scope,
                    &self.lua.globals(),
                    store,
                    &self.execution,
                    observer,
                    section,
                )
                .map_err(mlua::Error::external)?;
                let result = program
                    .load(&self.lua)
                    .map_err(mlua::Error::external)?
                    .call(());
                finish_log_phase(&self.lua, result)
            })
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

    #[expect(
        clippy::too_many_arguments,
        reason = "control-flow host fns are installed together for one Lua phase"
    )]
    fn run_loaded_with_control<E, F>(
        &self,
        program: &LuaProgram,
        observer: &dyn Observer,
        section: &str,
        tasks: &[LuaSectionHandle],
        execute_callback: Option<&E>,
        fanout_callback: Option<&F>,
        jump_enabled: bool,
    ) -> Result<LuaBlockResult>
    where
        E: Fn(Value, Option<String>) -> std::result::Result<String, Error>,
        F: Fn(String, String) -> std::result::Result<Vec<LuaFanoutResult>, Error>,
    {
        let store = self.store.as_ref().ok_or_else(|| {
            Error::Lua("section VM host values have not been injected".to_owned())
        })?;
        {
            let mut slot = self
                .jump_slot
                .lock()
                .map_err(|_| Error::Lua("jump slot was poisoned".to_owned()))?;
            *slot = None;
        }
        let jump_slot = Arc::clone(&self.jump_slot);
        let result = self.lua.scope(|scope| {
            install_log(
                &self.lua,
                scope,
                &self.execution,
                observer,
                section,
                &self.log_budget,
                &self.log_byte_budget,
            )
            .map_err(mlua::Error::external)?;
            install_store_table(
                &self.lua,
                scope,
                &self.lua.globals(),
                store,
                &self.execution,
                observer,
                section,
            )
            .map_err(mlua::Error::external)?;
            install_tasks_table(&self.lua, tasks).map_err(mlua::Error::external)?;
            if let Some(execute_callback) = execute_callback {
                let execute_fn = scope
                    .create_function(|_, (target, input): (Value, Option<String>)| {
                        execute_callback(target, input).map_err(mlua::Error::external)
                    })
                    .map_err(mlua::Error::external)?;
                self.lua
                    .globals()
                    .raw_set("execute", execute_fn)
                    .map_err(mlua::Error::external)?;
            }
            if jump_enabled {
                let jump_fn = scope
                    .create_function(move |_, target: Value| -> mlua::Result<()> {
                        let heading = resolve_section_target(target)?;
                        let mut slot = jump_slot
                            .lock()
                            .map_err(|_| mlua::Error::external("jump slot poisoned"))?;
                        *slot = Some(heading);
                        Err(mlua::Error::external("jump transfer"))
                    })
                    .map_err(mlua::Error::external)?;
                self.lua
                    .globals()
                    .raw_set("jump", jump_fn)
                    .map_err(mlua::Error::external)?;
            }
            if let Some(fanout_callback) = fanout_callback {
                let fanout_fn = scope
                    .create_function(|lua, (worker, list): (String, String)| {
                        let replies =
                            fanout_callback(worker, list).map_err(mlua::Error::external)?;
                        let table = lua.create_table_with_capacity(replies.len(), 0)?;
                        for (i, reply) in replies.into_iter().enumerate() {
                            table.raw_set(i + 1, reply)?;
                        }
                        Ok(table)
                    })
                    .map_err(mlua::Error::external)?;
                self.lua
                    .globals()
                    .raw_set("fanout", fanout_fn)
                    .map_err(mlua::Error::external)?;
            }
            let result = program
                .load(&self.lua)
                .map_err(mlua::Error::external)?
                .call(());
            finish_log_phase(&self.lua, result)
        });
        // Control-global cleanup runs on EVERY exit (jump, success, or ordinary
        // execution error), so a failing block never leaks live `jump`/`execute`/
        // `fanout`/`tasks` globals into a later phase (LUA-007). Cleanup failures
        // are combined with the execution outcome rather than discarded.
        let jump = self.take_jump();
        let cleanup = self.clear_control_globals();
        // Cleanup ran first, so control globals never leak (LUA-007) even when
        // the jump slot was poisoned; only then is the poison propagated
        // instead of being coerced into "no jump" (discarded-error-001).
        let jump = jump?;
        if let Some(heading) = jump {
            cleanup?;
            return Ok(LuaBlockResult::Jump(heading));
        }
        let returned = result.map_err(|error| program.map_runtime_error(&error));
        match (returned, cleanup) {
            // Execution error is the primary cause; it takes precedence.
            (Err(execution), _) => Err(execution),
            // Execution succeeded but cleanup failed: surface the cleanup error.
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Ok(values), Ok(())) => Ok(LuaBlockResult::Returned(scalar_return(values)?)),
        }
    }

    /// Clears the phase's control-flow globals, returning the first failure.
    ///
    /// Always attempts to clear every global even if an earlier clear fails, so
    /// no live control function is left installed for the next phase.
    fn clear_control_globals(&self) -> Result<()> {
        let globals = self.lua.globals();
        let mut first_error: Option<Error> = None;
        for name in ["jump", "execute", "fanout", "tasks"] {
            if let Err(error) = globals.raw_set(name, Value::Nil)
                && first_error.is_none()
            {
                first_error = Some(Error::lua(error));
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
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
                install_log(
                    &self.lua,
                    scope,
                    &self.execution,
                    observer,
                    section,
                    &self.log_budget,
                    &self.log_byte_budget,
                )
                .map_err(mlua::Error::external)?;
                install_store_table(
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
                finish_log_phase(&self.lua, result)
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

/// Installs `tools.calls` on the existing `tools` global as a read-only table.
///
/// Reading a known alias returns its current count from `counts`. Indexing an
/// unknown key raises a hard Lua error naming the bad key and listing the VM's
/// in-scope aliases. `declared` is the prompt-wide `tools.need` set used to
/// distinguish pure unknowns from declared-but-unscoped aliases.
/// Snapshot-reads always + H2 additions without closing the tool phase.
pub(crate) fn snapshot_tool_scope(
    bindings: &ToolBindings,
    runtime: &Mutex<ToolRuntime>,
) -> Result<ToolScope> {
    let runtime = runtime
        .lock()
        .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
    let aliases = bindings
        .always
        .iter()
        .chain(runtime.added.iter())
        .cloned()
        .collect::<Vec<_>>();
    let effective = aliases
        .iter()
        .map(|alias| binding_for_scope(bindings, &runtime, alias))
        .collect::<Result<Vec<_>>>()?;
    Ok(ToolScope {
        bindings: effective,
    })
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
