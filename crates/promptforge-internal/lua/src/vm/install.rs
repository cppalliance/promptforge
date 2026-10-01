//! Section VM setup: the captured alias globals, the Engine values and
//! persistent Engine globals, the control globals and yield shims, and the
//! `sys` and `prose` globals.

use super::{SectionVm, pack_sequence};
#[cfg(test)]
use crate::var_to_json;
use crate::{
    Access, Arc, Argv, Emitter, Error, Json, LuaModelHandle, LuaToolHandle, Mutex, Ordering,
    ProseState, Result, Value, guarded_var, install_compactors, install_log, install_messages,
    install_models, install_shim_prelude, install_store_table, install_tools,
    resolve_section_target, seal_sys,
};

impl SectionVm {
    /// Installs the captured tool and model alias globals.
    ///
    /// Each bound slot becomes a bare global holding its handle userdata.
    /// The engine calls this after [`replay_shared`](Self::replay_shared), so
    /// a declared alias wins over a same-named shared global; the raw install
    /// also bypasses any metatable the shared library set on `_G`. The raw
    /// install never replaces an Engine global only because the parser refuses
    /// an alias on [`RESERVED_NAMES`](crate::RESERVED_NAMES) and one name
    /// under both `tools` and `models`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if a handle cannot be created or installed, or
    /// a shared set's mutex is poisoned.
    pub fn install_captured_bindings(&self) -> Result<()> {
        let globals = self.lua.globals();
        {
            let tools = self
                .bound_tools
                .lock()
                .map_err(|_| Error::Lua("tool set mutex was poisoned".to_owned()))?;
            for binding in tools.bindings() {
                let handle = LuaToolHandle::from_binding(
                    binding.alias(),
                    binding.description(),
                    binding.id(),
                );
                let userdata = self.lua.create_userdata(handle).map_err(Error::lua)?;
                globals
                    .raw_set(binding.alias(), userdata)
                    .map_err(Error::lua)?;
            }
        }
        {
            let models = self
                .bound_models
                .lock()
                .map_err(|_| Error::Lua("model set mutex was poisoned".to_owned()))?;
            for binding in models.bindings() {
                // Handles are plain frozen userdata in every mode: invocation is
                // namespace-only (`models.infer(handle, prompt)`), so no
                // shim-wrapped proxy is needed.
                let handle = LuaModelHandle::from_binding(binding);
                let userdata = self.lua.create_userdata(handle).map_err(Error::lua)?;
                globals
                    .raw_set(binding.alias(), userdata)
                    .map_err(Error::lua)?;
            }
        }
        Ok(())
    }

    /// Installs the section's Engine values, ahead of the shared replay.
    ///
    /// This operation may be called exactly once. The store callbacks own a
    /// clone of the run-scoped store. `log` and `store` are installed once for
    /// the section's whole lifecycle by
    /// [`install_host_apis`](Self::install_host_apis), which captures a
    /// clone of the emitter rather than a per-chunk borrow.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if Engine values cannot be bridged or if Engine
    /// values were already injected.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_lua::SectionVm;
    /// use promptforge_types::emitter::{DebugMode, Emitter, EventSink};
    /// use promptforge_types::untrusted::GuardNonce;
    ///
    /// let nonce = GuardNonce::from_seed(1);
    /// let emitter = Emitter::root(EventSink::default(), "example-run", DebugMode::Off);
    /// let vfs = promptforge_vfs::VfsRef::default();
    /// let access = std::sync::Arc::new(
    ///     vfs.acquire(promptforge_vfs::Origin::new("vm example"))?,
    /// );
    /// let mut vm = SectionVm::new(&nonce, &emitter, "Example")?;
    /// vm.inject_host("input", &serde_json::json!({ "id": 1 }), &access)?;
    /// vm.teardown(&emitter, "Example");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn inject_host(&mut self, args: &str, sys: &Json, access: &Arc<Access>) -> Result<()> {
        self.inject_host_with_var(args, sys, access, None, Argv::Frozen(None))
    }

    /// Installs Engine values while seeding `var` from an earlier VM.
    ///
    /// The `var` global is a guarded proxy (see `guarded_var`): writes are
    /// validated for JSON-representability at the assigning line, and the
    /// hidden data table behind it is what [`var`](Self::var) reads back.
    /// `access` is the chain step's VFS capability: the `store` table's
    /// closures share it, so a fanout arm's store ops are attributed to
    /// the arm's spawned identity, and a conflicting access from an
    /// identity not ordered after the arm's surfaces as a write race.
    ///
    /// `argv` is the parsed form of the args string, installed per its
    /// [`Argv`] mode: writable for the H1 pass (whose repaired value the
    /// executor reads back with [`argv_json`](Self::argv_json)), frozen for
    /// every other section.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if Engine values cannot be bridged or were
    /// already injected.
    #[expect(
        clippy::similar_names,
        reason = "args and argv are the spec'd global names; the pair is intentional"
    )]
    pub fn inject_host_with_var(
        &mut self,
        args: &str,
        sys: &Json,
        access: &Arc<Access>,
        initial_var: Option<&Json>,
        argv: Argv<'_>,
    ) -> Result<()> {
        if self.host_injected {
            return Err(Error::Lua(
                "section VM host values were already injected".to_owned(),
            ));
        }

        let globals = self.lua.globals();
        globals.raw_set("args", args).map_err(Error::lua)?;
        match argv {
            Argv::Writable(value) => crate::argv::install_writable(&self.lua, value)?,
            Argv::Frozen(value) => crate::argv::install_frozen(&self.lua, value)?,
        }
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
        install_tools(
            &self.lua,
            &globals,
            &self.bound_tools,
            &self.tool_runtime,
            &self.local_tools,
        )?;
        install_models(
            &self.lua,
            &globals,
            &self.bound_models,
            &self.model_runtime,
            self.raw_model_ids,
        )?;
        install_messages(&self.lua, &globals)?;
        install_compactors(&self.lua, &globals)?;
        self.access = Some(Arc::clone(access));
        self.host_injected = true;
        Ok(())
    }

    /// Installs `log` and `store` as persistent globals for the section's
    /// whole lifecycle.
    ///
    /// Called once after [`inject_host_with_var`](Self::inject_host_with_var).
    /// The closures capture owned strings, a clone of the emitter, and Arc
    /// clones of the log budget counters and the store view, so they stay
    /// valid across every chunk this VM runs without a live [`mlua::Scope`].
    /// The direct store closures record a claims-model conflict into the
    /// VM's slot for the executor to read with
    /// [`take_store_conflict`](Self::take_store_conflict).
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if Engine values have not been injected or the
    /// globals cannot be installed, or [`Error::Store`] if the handle
    /// declares no store.
    pub fn install_host_apis(&self, emitter: &Emitter, section: &str) -> Result<()> {
        let access = self.access.as_ref().ok_or_else(|| {
            Error::Lua("section VM host values have not been injected".to_owned())
        })?;
        install_log(
            &self.lua,
            emitter,
            section,
            &self.log_budget,
            &self.log_byte_budget,
        )?;
        install_store_table(
            &self.lua,
            &self.lua.globals(),
            access,
            emitter,
            section,
            &self.store_conflicts,
        )
    }

    /// Takes the claims-model conflict the direct store closures recorded
    /// during the shared replay, clearing the slot. The executor reads it
    /// when [`replay_shared`](Self::replay_shared) returns, so a conflict
    /// author code caught with `pcall` still ends the run with a
    /// determinism violation.
    #[must_use]
    pub fn take_store_conflict(&self) -> Option<String> {
        self.store_conflicts
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    /// Installs `call`, `jump`, and `list_from_section` as persistent
    /// globals for the section's whole lifecycle.
    ///
    /// Called once by `promptforge-engine` after Engine injection. The
    /// callbacks own their run context, so the closures stay valid across
    /// every chunk this VM runs without a live [`mlua::Scope`]. The `jump`
    /// closure captures a clone of the VM's jump slot; the slot is reset
    /// before each chunk and read after it by the control-run path. The
    /// `call` closure snapshots this VM's `var` at call time (reading the
    /// hidden data table through the in-scope `&Lua`) and hands the JSON to
    /// its callback, so a contained chain seeds from a clone and its writes
    /// never reach this VM.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if any global cannot be installed.
    #[cfg(test)]
    pub(crate) fn install_control_globals<E, L>(
        &self,
        call_callback: E,
        list_callback: L,
    ) -> Result<()>
    where
        E: Fn(Value, Option<String>, Json) -> std::result::Result<String, Error> + Send + 'static,
        L: Fn(String) -> std::result::Result<Vec<String>, Error> + Send + 'static,
    {
        let globals = self.lua.globals();
        let call_fn = self
            .lua
            .create_function(move |lua, (target, input): (Value, Option<String>)| {
                let var = var_to_json(lua).map_err(mlua::Error::external)?;
                call_callback(target, input, var).map_err(mlua::Error::external)
            })
            .map_err(Error::lua)?;
        globals.raw_set("call", call_fn).map_err(Error::lua)?;
        self.install_jump_global(&globals)?;
        self.install_list_global(&globals, list_callback)
    }

    /// Installs the scheduler-mode control surface: `jump` and
    /// `list_from_section` as Rust callbacks (neither suspends).
    ///
    /// The suspending calls (`models.infer`, `call`, `fanout`,
    /// `tools.call`, `tasks.*`) are the yield shims installed by
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

    /// Installs the coroutine yield shims (`models.infer`, `call`,
    /// `fanout`, `tools.call`, the `tasks` namespace). `max_tool_iterations`
    /// is the run's resolved round cap for the `models.loop` shim a section
    /// install adds afterward; a VM that never installs the loop shim
    /// passes any value.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the shim prelude cannot install.
    pub fn install_coro_shims(&mut self, max_tool_iterations: usize) -> Result<()> {
        install_shim_prelude(
            &self.lua,
            max_tool_iterations,
            &self.local_handler_depth,
            &self.instruction_budget,
        )
    }

    fn install_jump_global(&self, globals: &mlua::Table) -> Result<()> {
        let jump_slot = Arc::clone(&self.jump_slot);
        let local_handler_depth = Arc::clone(&self.local_handler_depth);
        let jump_fn = self
            .lua
            .create_function(move |_, target: Value| -> mlua::Result<()> {
                if local_handler_depth.load(Ordering::Relaxed) > 0 {
                    return Err(mlua::Error::external(
                        "jump is unavailable inside a local tool handler: return a value from \
                         the handler and call jump from the block after the tool call returns",
                    ));
                }
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

    /// Replaces the sealed Lua `sys` global after scope close.
    ///
    /// Engine injection must have run first. Used to expose `sys.model` once the
    /// section's model binding is fixed.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if Engine values have not been injected or the
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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "exercised by the lua module's poisoned-slot test")
    )]
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

    /// Installs the pending Markdown buffer as this VM's fresh read-only
    /// lazy `prose` global, replacing any previous pair's render.
    ///
    /// The executor calls this before each Lua coroutine starts. `render`
    /// runs at most once, on the first runtime read of `prose`, with the
    /// section state snapshot ([`ProseState`]); its result is memoized for
    /// later reads. Assigning to `prose` raises, and `{{ prose }}` inside
    /// the template is rejected as recursive.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the read cannot be built or the `_G` guard
    /// cannot record it.
    pub fn install_lazy_prose<F>(&self, render: F) -> Result<()>
    where
        F: Fn(ProseState) -> mlua::Result<String> + Send + Sync + 'static,
    {
        crate::prose::install(&self.lua, &self.sys_live, render)
    }
}
