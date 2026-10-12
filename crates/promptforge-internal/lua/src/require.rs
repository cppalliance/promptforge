//! PromptForge's `require`: a section VM's loader for library sections.
//!
//! `require '## heading'` asks the caller's resolver for a [`Library`],
//! loads its compiled blocks into the VM's one environment, and runs them
//! in order inside the calling block's coroutine, so a library block's
//! suspending calls are ordinary yields of that block. The loader is Lua
//! (`__impl_require.lua`), because a Rust function cannot call one that
//! yields; the Rust helpers here resolve a library, load its blocks, track
//! which library is loading, step the depth counter `jump` reads, and
//! report each load's boundaries.
//!
//! The standard package library stays unloaded: `require` reaches
//! sections only, through the resolver, never files or native modules.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use mlua::{AnyUserData, Function, Value};
use promptforge_types::emitter::{Emitter, LibraryLoad};

use super::{Error, Library, LibraryKey, Lua, LuaProgram, Result, SharedSource};
use crate::error_value::install_error_value;

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

/// Installs `require` as a global over `resolver`, reporting each first
/// load through a clone of `emitter` under `section` and stepping `depth`
/// while a library loads.
///
/// # Errors
/// Returns [`Error::Lua`] if the loader chunk or its helpers cannot be
/// built or installed.
pub(crate) fn install<R, E>(
    lua: &Lua,
    resolver: R,
    emitter: &Emitter,
    section: &str,
    depth: &Arc<AtomicU32>,
) -> Result<()>
where
    R: Fn(String, Option<LibraryKey>) -> std::result::Result<Library, E> + Send + Sync + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let loading = Loading::default();
    let resolve = resolve_capture(lua, resolver, &loading)?;
    let begin_load = begin_load_capture(lua, &loading, depth, emitter, section)?;
    let end_load = end_load_capture(lua, &loading, depth, emitter, section)?;
    let error_value = install_error_value(lua).map_err(Error::lua)?;
    let require: Function = REQUIRE_PROGRAM
        .as_ref()
        .map_err(Error::shared)?
        .load(lua)?
        .call((resolve, begin_load, end_load, error_value))
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
        let from = lock(&loading)?.last().map(|(key, _)| key.clone());
        let library = resolver(heading, from).map_err(mlua::Error::external)?;
        let identity = library.key.identity().to_owned();
        let library_heading = library.heading.clone();
        Ok((identity, library_heading, lua.create_any_userdata(library)?))
    })
    .map_err(Error::lua)
}

/// Builds `begin_load(library)`: reports the load's start, loads the
/// library's blocks, and marks it as the one loading, returning the blocks
/// in order. A block that fails to load reports the load as failed and
/// leaves nothing marked.
fn begin_load_capture(
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
    lua.create_function(move |lua, library: AnyUserData| {
        let library = library.take::<Library>()?;
        let mut loading = lock(&loading)?;
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
        loading.push((library.key, library.heading));
        depth.fetch_add(1, Ordering::Relaxed);
        Ok(blocks)
    })
    .map_err(Error::lua)
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
        let Some((_, heading)) = lock(&loading)?.pop() else {
            return Err(mlua::Error::external(Error::Internal(
                "require ended a library load that never began",
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

/// Locks the VM's loads, mapping a poisoned lock to a Lua error.
fn lock(loading: &Loading) -> mlua::Result<MutexGuard<'_, Vec<(LibraryKey, String)>>> {
    loading.lock().map_err(|_| {
        mlua::Error::external(Error::Lua(
            "require's library-load state was poisoned".to_owned(),
        ))
    })
}

#[cfg(test)]
#[path = "require-tests.rs"]
mod tests;
