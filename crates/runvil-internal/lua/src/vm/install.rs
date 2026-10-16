//! Section VM setup after Engine injection: the persistent Engine globals,
//! the control globals and yield shims, `require`, and the `sys` and
//! `prose` globals.

use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use mlua::Value;
use runvil_types::emitter::Emitter;
use serde_json::Value as Json;

use super::{SYS_LIVE_POISONED, SectionVm, pack_sequence};
use crate::coro::install_shim_prelude;
use crate::engine_globals::{install_log, install_store_table};
use crate::error::{Error, Result};
use crate::handles::{Library, LibraryKey, SectionText, resolve_section_target};
use crate::prose::ProseState;
use crate::sys::seal_sys;
#[cfg(test)]
use crate::sys::var_to_json;

impl SectionVm {
    /// Installs `log` and `store` as persistent globals for the section's
    /// whole lifecycle.
    ///
    /// Called once after Engine injection.
    /// The closures capture owned strings, a clone of the emitter, and Arc
    /// clones of the log budget counters and the store view, so they stay
    /// valid across every chunk this VM runs without a live [`mlua::Scope`].
    /// The direct store closures record a claims-model conflict into the
    /// VM's slot for the executor to read with
    /// [`take_store_conflict`](Self::take_store_conflict).
    ///
    /// # Errors
    /// Returns [`Error::LuaRuntime`] if the globals cannot be installed, or
    /// [`Error::Store`] if the handle declares no store.
    pub fn install_engine_globals(&self, emitter: &Emitter, section: &str) -> Result<()> {
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
            &self.state.access,
            emitter,
            section,
            &self.store_conflicts,
        )
    }

    /// Takes the claims-model conflict the direct store closures recorded
    /// while the Plugin preludes installed, clearing the slot. The executor
    /// reads it once the preludes return, so a conflict a prelude caught
    /// with `pcall` still ends the run with a determinism violation.
    #[must_use]
    pub fn take_store_conflict(&self) -> Option<String> {
        // A panic elsewhere cannot leave the slot half-written, so a
        // poisoned lock still holds the conflict to hand over.
        self.store_conflicts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Shared store-conflict slot, for the poisoned-slot test.
    #[cfg(test)]
    pub(crate) fn store_conflicts_handle(&self) -> Arc<std::sync::Mutex<Option<String>>> {
        Arc::clone(&self.store_conflicts)
    }

    /// Installs `call`, `jump`, and the `prompt` reads (`prompt.section`
    /// and `prompt.list`) as persistent globals for the section's whole
    /// lifecycle.
    ///
    /// Called once by `runvil-engine` after Engine injection. The
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
    /// Returns [`Error::LuaRuntime`] if any global cannot be installed.
    #[cfg(test)]
    pub(crate) fn install_control_globals<E, R>(&self, call_callback: E, read: R) -> Result<()>
    where
        E: Fn(Value, Option<String>, Json) -> std::result::Result<String, Error> + Send + 'static,
        R: Fn(String) -> std::result::Result<Option<SectionText>, Error> + Send + Sync + 'static,
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
        self.install_prompt_table(&globals, read)
    }

    /// Installs the scheduler-mode control surface: `jump` and the
    /// `prompt` reads as Rust callbacks (none suspends).
    ///
    /// The suspending calls (`models.infer`, `call`, `tasks.fanout`,
    /// `tools.call`, `tasks.*`) are the yield shims installed by
    /// [`install_coro_shims`](Self::install_coro_shims).
    ///
    /// # Errors
    /// Returns [`Error::LuaRuntime`] if any global cannot be installed.
    pub fn install_scheduler_control_globals<R, E>(&self, read: R) -> Result<()>
    where
        R: Fn(String) -> std::result::Result<Option<SectionText>, E> + Send + Sync + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let globals = self.lua.globals();
        self.install_jump_global(&globals)?;
        self.install_prompt_table(&globals, read)
    }

    /// Installs the coroutine yield shims (`models.infer`, `call`,
    /// `tasks.fanout`, `tools.call`, the `tasks` namespace).
    /// `max_tool_iterations` is the run's resolved round cap for the
    /// `models.loop` shim a section install adds afterward; a VM that never
    /// installs the loop shim passes any value.
    ///
    /// # Errors
    /// Returns [`Error::LuaRuntime`] if the shim prelude cannot install.
    pub fn install_coro_shims(&mut self, max_tool_iterations: usize) -> Result<()> {
        install_shim_prelude(
            &self.lua,
            max_tool_iterations,
            &self.local_handler_depth,
            &self.instruction_budget,
        )
    }

    /// Installs Runvil's `require` as a persistent global for the
    /// section's whole lifecycle.
    ///
    /// `resolver` finds a library from the heading as written. While a
    /// library loads, a `require` passes that library's key, so the
    /// library's own helpers resolve from its position wherever it is
    /// required from; every other call passes `None`. An out-of-reach or
    /// malformed heading is the resolver's error, raised at the call. Each
    /// first load reports through a clone of `emitter` under `section`, and
    /// `jump` refuses while any library loads.
    ///
    /// While a library block runs, its `prose` is its own: a lazy read of
    /// the block's prose template, rendered at most once through `render`
    /// with the template and the same [`ProseState`] snapshot
    /// [`install_lazy_prose`](Self::install_lazy_prose)'s render takes, read
    /// from the caller's environment. The caller's `prose`, memo included,
    /// comes back when the block ends, however it ends. A failure anywhere
    /// in library code maps to the library block's location and prompt
    /// line.
    ///
    /// # Errors
    /// Returns [`Error::LuaRuntime`] if the loader cannot install.
    pub fn install_require<R, E, P>(
        &self,
        resolver: R,
        render: P,
        emitter: &Emitter,
        section: &str,
    ) -> Result<()>
    where
        R: Fn(String, Option<LibraryKey>) -> std::result::Result<Library, E>
            + Send
            + Sync
            + 'static,
        E: std::error::Error + Send + Sync + 'static,
        P: Fn(&str, ProseState<'_>) -> mlua::Result<String> + Send + Sync + 'static,
    {
        crate::require::install(
            &self.lua,
            resolver,
            render,
            emitter,
            section,
            &crate::require::VmShare {
                depth: &self.library_load_depth,
                sys_live: &self.sys_live,
                programs: &self.library_programs,
            },
        )
    }

    fn install_jump_global(&self, globals: &mlua::Table) -> Result<()> {
        let jump_slot = Arc::clone(&self.jump_slot);
        let local_handler_depth = Arc::clone(&self.local_handler_depth);
        let library_load_depth = Arc::clone(&self.library_load_depth);
        let jump_fn = self
            .lua
            .create_function(move |_, target: Value| -> mlua::Result<()> {
                if local_handler_depth.load(Ordering::Relaxed) > 0 {
                    return Err(mlua::Error::external(
                        "jump is unavailable inside a local tool handler: return a value from \
                         the handler and call jump from the block after the tool call returns",
                    ));
                }
                if library_load_depth.load(Ordering::Relaxed) > 0 {
                    return Err(mlua::Error::external(
                        "jump is unavailable while a library loads: return a value from the \
                         library and call jump from the section after require returns",
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

    fn install_prompt_table<R, E>(&self, globals: &mlua::Table, read: R) -> Result<()>
    where
        R: Fn(String) -> std::result::Result<Option<SectionText>, E> + Send + Sync + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let read = Arc::new(read);
        let read_markdown = Arc::clone(&read);
        let section = self
            .lua
            .create_function(move |_, target: Value| {
                let text = read_markdown(resolve_section_target(target)?)
                    .map_err(mlua::Error::external)?;
                Ok(text.map(|text| text.markdown))
            })
            .map_err(Error::lua)?;
        let list = self
            .lua
            .create_function(move |lua, target: Value| {
                let heading = resolve_section_target(target)?;
                match read(heading.clone()).map_err(mlua::Error::external)? {
                    None => Ok(Value::Nil),
                    Some(text) if text.items.is_empty() => Err(mlua::Error::external(format!(
                        "section `{}` has no pre-parsed items",
                        heading.trim()
                    ))),
                    Some(text) => pack_sequence(lua, text.items).map(Value::Table),
                }
            })
            .map_err(Error::lua)?;
        let prompt = self.lua.create_table().map_err(Error::lua)?;
        prompt.raw_set("section", section).map_err(Error::lua)?;
        prompt.raw_set("list", list).map_err(Error::lua)?;
        globals.raw_set("prompt", prompt).map_err(Error::lua)
    }

    /// Replaces the sealed Lua `sys` global after scope close, to expose
    /// `sys.model` once the section's model binding is fixed.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] if the live slot is poisoned, or the
    /// install failure if the sealed table cannot be installed.
    pub fn re_seal_sys(&self, sys: &Json) -> Result<()> {
        let globals = self.lua.globals();
        let sys_table = seal_sys(&self.lua, sys)?;
        globals.raw_set("sys", sys_table).map_err(Error::lua)?;
        let mut live = self
            .sys_live
            .lock()
            .map_err(|_| Error::Internal(SYS_LIVE_POISONED.into()))?;
        *live = Some(sys.clone());
        Ok(())
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
        F: Fn(ProseState<'_>) -> mlua::Result<String> + Send + Sync + 'static,
    {
        crate::prose::install(&self.lua, &self.sys_live, render)
    }
}

impl<S> SectionVm<S> {
    /// Snapshot of the live sealed `sys` JSON, or `fallback` when unset.
    ///
    /// Distinguishes the two non-value states rather than collapsing both to
    /// `fallback`: an *unset* live slot (before Engine injection fills it) is
    /// a legitimate state and yields `Ok(fallback)`, while a *poisoned* lock
    /// is a real failure and yields [`Error::Internal`] instead of silently
    /// masquerading as the fallback.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the live `sys` mutex is poisoned.
    pub fn current_sys(&self, fallback: &Json) -> Result<Json> {
        let guard = self
            .sys_live
            .lock()
            .map_err(|_| Error::Internal(SYS_LIVE_POISONED.into()))?;
        Ok(guard.clone().unwrap_or_else(|| fallback.clone()))
    }

    /// Shared live `sys` JSON, for the poisoned-slot test.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn sys_live_handle(&self) -> Arc<std::sync::Mutex<Option<Json>>> {
        Arc::clone(&self.sys_live)
    }
}
