//! PromptForge's `require`: a section VM's loader for library sections.
//!
//! `require '## heading'` asks the caller's resolver for a [`Library`],
//! loads its compiled blocks into the VM's one environment, and runs them
//! in order inside the calling block's coroutine, so a library block's
//! suspending calls are ordinary yields of that block. The loader is Lua
//! (`__impl_require.lua`), because a Rust function cannot call one that
//! yields; the Rust helpers here resolve a library, load its blocks, track
//! which library is loading, step the depth counter `jump` reads, report
//! each load's boundaries, swap each block's own `prose` in and the
//! caller's back out, and keep every loaded block's program on the VM for
//! error mapping.
//!
//! The standard package library stays unloaded: `require` reaches
//! sections only, through the resolver, never files or native modules.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use mlua::{AnyUserData, Function, Lua, Value};
use promptforge_types::emitter::{Emitter, LibraryLoad};
use serde_json::Value as Json;

use crate::SharedSource;
use crate::error::{Error, Result};
use crate::error_value::install_error_value;
use crate::handles::{Library, LibraryKey};
use crate::program::LuaProgram;
use crate::prose::ProseState;

/// The loader chunk's name, `@`-prefixed as the shim chunks' names are.
const REQUIRE_CHUNK_NAME: &str = "@crates/promptforge-internal/lua/src/__impl_require.lua";

/// The loader source, embedded verbatim so chunk line 1 is file line 1.
const REQUIRE_SOURCE: &str = include_str!("__impl_require.lua");

/// The loader program, compiled once and loaded per VM, under the shim
/// programs' failure contract.
static REQUIRE_PROGRAM: LazyLock<std::result::Result<LuaProgram, SharedSource>> =
    LazyLock::new(|| {
        LuaProgram::compile_internal(REQUIRE_SOURCE, REQUIRE_CHUNK_NAME)
            .map_err(crate::detail::shared_source_new)
    });

/// The loads under way on one VM, outermost first: each library's key,
/// which a `require` made during its load passes to the resolver, and its
/// heading, which its reports name.
type Loading = Arc<Mutex<Vec<(LibraryKey, String)>>>;

/// The section VM's state `require` works on.
pub(crate) struct VmShare<'a> {
    /// The counter `jump` reads, stepped while a library loads.
    pub(crate) depth: &'a Arc<AtomicU32>,
    /// The live `sys` mirror a library block's `prose` renders from.
    pub(crate) sys_live: &'a Arc<Mutex<Option<Json>>>,
    /// Every library block's program loaded into the VM, which a block
    /// failure maps through.
    pub(crate) programs: &'a Arc<Mutex<Vec<LuaProgram>>>,
}

/// Installs `require` as a global over `resolver`, rendering each library
/// block's `prose` template through `render`, reporting each first load
/// through a clone of `emitter` under `section`, and working on `vm`.
///
/// # Errors
/// Returns [`Error::LuaRuntime`] if the loader chunk or its helpers cannot be
/// built or installed.
pub(crate) fn install<R, E, P>(
    lua: &Lua,
    resolver: R,
    render: P,
    emitter: &Emitter,
    section: &str,
    vm: &VmShare<'_>,
) -> Result<()>
where
    R: Fn(String, Option<LibraryKey>) -> std::result::Result<Library, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
    P: Fn(&str, ProseState) -> mlua::Result<String> + Send + Sync + 'static,
{
    let loading = Loading::default();
    let resolve = resolve_capture(lua, resolver, &loading)?;
    let begin_load = begin_load_capture(lua, &loading, vm, emitter, section)?;
    let end_load = end_load_capture(lua, &loading, vm.depth, emitter, section)?;
    let (swap_prose, restore_prose) = prose_captures(lua, render, vm.sys_live)?;
    let error_value = install_error_value(lua).map_err(Error::lua)?;
    let require: Function = REQUIRE_PROGRAM
        .as_ref()
        .map_err(Error::shared)?
        .load(lua)?
        .call((
            resolve,
            begin_load,
            end_load,
            swap_prose,
            restore_prose,
            error_value,
        ))
        .map_err(Error::lua)?;
    lua.globals()
        .raw_set("require", require)
        .map_err(Error::lua)
}

/// Builds `resolve(heading)`: the resolver's library for `heading`, asked
/// with the key of the library loading now, if any, and handed back as
/// its identity, its heading, and the library itself for `begin_load`.
fn resolve_capture<R, E>(lua: &Lua, resolver: R, loading: &Loading) -> Result<Function>
where
    R: Fn(String, Option<LibraryKey>) -> std::result::Result<Library, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let loading = Arc::clone(loading);
    lua.create_function(move |lua, heading: Value| {
        let heading = match heading {
            Value::String(heading) => heading.to_str()?.to_owned(),
            other => {
                return Err(mlua::Error::external(format!(
                    "require takes a section heading string such as '## helpers', got {}",
                    other.type_name()
                )));
            }
        };
        let from = lock(&loading, "library-load state")?
            .last()
            .map(|(key, _)| key.clone());
        let library = resolver(heading, from).map_err(mlua::Error::external)?;
        let identity = library.key.identity().to_owned();
        let library_heading = library.heading.clone();
        Ok((identity, library_heading, lua.create_any_userdata(library)?))
    })
    .map_err(Error::lua)
}

/// Builds `begin_load(library)`: reports the load's start, loads the
/// library's blocks, keeps their programs on the VM, and marks the library
/// as the one loading, returning the blocks and their prose templates in
/// order. A block that fails to load reports the load as failed and leaves
/// nothing marked.
fn begin_load_capture(
    lua: &Lua,
    loading: &Loading,
    vm: &VmShare<'_>,
    emitter: &Emitter,
    section: &str,
) -> Result<Function> {
    let loading = Arc::clone(loading);
    let depth = Arc::clone(vm.depth);
    let programs = Arc::clone(vm.programs);
    let emitter = emitter.clone();
    let section = section.to_owned();
    lua.create_function(move |lua, library: AnyUserData| {
        let library = library.take::<Library>()?;
        emitter.library_load(&section, &library.heading, LibraryLoad::Started);
        let blocks = library
            .blocks
            .iter()
            .map(|block| block.program.load(lua))
            .collect::<Result<Vec<Function>>>();
        let blocks = match blocks {
            Ok(blocks) => blocks,
            Err(error) => {
                emitter.library_load(&section, &library.heading, LibraryLoad::Failed);
                return Err(mlua::Error::external(error));
            }
        };
        let mut programs = lock(&programs, "loaded library programs")?;
        let mut templates = Vec::with_capacity(library.blocks.len());
        for block in library.blocks {
            templates.push(block.prose);
            if !programs.contains(&block.program) {
                programs.push(block.program);
            }
        }
        drop(programs);
        let mut loading = match lock(&loading, "library-load state") {
            Ok(loading) => loading,
            Err(error) => {
                emitter.library_load(&section, &library.heading, LibraryLoad::Failed);
                return Err(error);
            }
        };
        loading.push((library.key, library.heading));
        depth.fetch_add(1, Ordering::Relaxed);
        Ok((blocks, templates))
    })
    .map_err(Error::lua)
}

/// Builds `swap_prose(template)`, which installs a library block's own
/// lazy `prose` over `template`, rendered through `render`, and returns the
/// read it replaced, and `restore_prose(saved)`, which puts that read back.
fn prose_captures<P>(
    lua: &Lua,
    render: P,
    sys_live: &Arc<Mutex<Option<Json>>>,
) -> Result<(Function, Function)>
where
    P: Fn(&str, ProseState) -> mlua::Result<String> + Send + Sync + 'static,
{
    let render = Arc::new(render);
    let sys_live = Arc::clone(sys_live);
    let swap = lua
        .create_function(move |lua, template: String| {
            let render = Arc::clone(&render);
            crate::prose::swap(lua, &sys_live, move |state| render(&template, state))
                .map_err(mlua::Error::external)
        })
        .map_err(Error::lua)?;
    let restore = lua
        .create_function(|lua, saved: Value| {
            crate::prose::restore(lua, saved).map_err(mlua::Error::external)
        })
        .map_err(Error::lua)?;
    Ok((swap, restore))
}

/// Builds `end_load(finished)`: takes back the innermost load's mark and
/// reports it as succeeded or failed.
fn end_load_capture(
    lua: &Lua,
    loading: &Loading,
    depth: &Arc<AtomicU32>,
    emitter: &Emitter,
    section: &str,
) -> Result<Function> {
    let loading = Arc::clone(loading);
    let depth = Arc::clone(depth);
    let emitter = emitter.clone();
    let section = section.to_owned();
    lua.create_function(move |_, finished: bool| {
        let Some((_, heading)) = lock(&loading, "library-load state")?.pop() else {
            return Err(mlua::Error::external(Error::Internal(
                "require ended a library load that never began".into(),
            )));
        };
        depth.fetch_sub(1, Ordering::Relaxed);
        let phase = if finished {
            LibraryLoad::Succeeded
        } else {
            LibraryLoad::Failed
        };
        emitter.library_load(&section, &heading, phase);
        Ok(())
    })
    .map_err(Error::lua)
}

/// Locks one of `require`'s shared slots, `what`, mapping a poisoned lock
/// to an internal error.
fn lock<'a, T>(slot: &'a Mutex<T>, what: &str) -> mlua::Result<MutexGuard<'a, T>> {
    slot.lock().map_err(|_| {
        mlua::Error::external(Error::Internal(
            format!("require's {what} was poisoned").into(),
        ))
    })
}

#[cfg(test)]
#[path = "require-tests.rs"]
mod tests;
