//! The coroutine-protocol shim layer: per-VM Lua yield wrappers for the
//! suspending host calls.
//!
//! Yield cannot cross the C boundary, so `models.infer`, `handle:infer`,
//! `execute`, `fanout`, and `tool_call` are Lua shims (source in `__impl_coro.lua` beside this
//! file) that `coroutine.yield` a request table and interpret the two
//! resume values as the `(ok, result)` envelope; coroutine driving itself
//! (`Thread::create`/`resume`) is pure Rust in the scheduler. The source is
//! pulled in with `include_str!` so chunk line 1 is file line 1, compiled
//! once through the usual [`LuaProgram`] machinery, and loaded per VM. The
//! chunk is named with an `@` prefix, so PUC's `luaO_chunkid` renders shim
//! frames as verbatim `file:line:` references with no `[string "..."]`
//! wrapper, and the line mapper (`program.rs`) never touches them.

use std::sync::LazyLock;

use mlua::{Function, Table, Value};

use super::{Error, Lua, LuaModelHandle, LuaProgram, Result, StdLib, var_snapshot_table};

/// The shim chunk's name: `@`-prefixed so PUC renders it verbatim as a file
/// path, making unexpected shim errors clickable `file:line:` references.
const SHIM_CHUNK_NAME: &str = "@crates/promptforge-core/src/lua/__impl_coro.lua";

/// The shim source, embedded verbatim so chunk line 1 is file line 1.
const SHIM_SOURCE: &str = include_str!("__impl_coro.lua");

/// The registry key for the shim's `wrap_handle`, stashed at install so the
/// captured model alias globals (which install last) wrap too.
const WRAP_HANDLE_REGISTRY: &str = "promptforge.impl_coro.wrap_handle";

/// The registry key for the shim's `infer`, stashed by the live H1 base
/// install so each H1 block's fresh live models table can be wrapped.
const INFER_REGISTRY: &str = "promptforge.impl_coro.infer";

/// The live H1 wrap chunk's name: `@`-prefixed so PUC renders it verbatim
/// as a file path, like the main shim chunk.
const H1_SHIM_CHUNK_NAME: &str = "@crates/promptforge-core/src/lua/__impl_coro_h1.lua";

/// The live H1 wrap source, embedded verbatim so chunk line 1 is file line 1.
const H1_SHIM_SOURCE: &str = include_str!("__impl_coro_h1.lua");

/// The shim program, compiled once and loaded per VM. Compilation of the
/// bundled source fails only on a crate bug, so the payload is the error's
/// display string (the crate `Error` is not `Clone`).
static SHIM_PROGRAM: LazyLock<std::result::Result<LuaProgram, String>> = LazyLock::new(|| {
    LuaProgram::compile_internal(SHIM_SOURCE, SHIM_CHUNK_NAME).map_err(|error| error.to_string())
});

/// The live H1 wrap program, compiled once and loaded per H1 block step.
static H1_SHIM_PROGRAM: LazyLock<std::result::Result<LuaProgram, String>> = LazyLock::new(|| {
    LuaProgram::compile_internal(H1_SHIM_SOURCE, H1_SHIM_CHUNK_NAME)
        .map_err(|error| error.to_string())
});

/// Installs the yield shims on a VM whose host tables already exist.
///
/// Scheduler-mode VMs load the coroutine standard library for the shim's
/// `yield` capture (legacy VMs keep exactly `STRING | TABLE | MATH`); the
/// `coroutine` global is stripped again before returning, so author code
/// cannot yield directly and a hand-rolled yield fails the driver's strict
/// validation. The `models` table is passed to the shim chunk as an
/// argument, so the chunk never reads a global; the chunk shims
/// `models.infer` and wraps the `models.use`/`models.get` returns, and the
/// `execute`/`fanout`/`tool_call` shims and `wrap_handle` come back for the
/// host to install.
///
/// # Errors
/// Returns [`Error::Lua`] if the coroutine library, the shim chunk, or any
/// install step fails.
pub(crate) fn install_shim_prelude(lua: &Lua) -> Result<()> {
    lua.load_std_libs(StdLib::COROUTINE).map_err(Error::lua)?;
    let globals = lua.globals();
    let coroutine: Table = globals.raw_get("coroutine").map_err(Error::lua)?;
    let yield_fn: Function = coroutine.raw_get("yield").map_err(Error::lua)?;
    let var_snapshot = lua
        .create_function(|lua, ()| var_snapshot_table(lua).map_err(mlua::Error::external))
        .map_err(Error::lua)?;
    let models: Table = globals.raw_get("models").map_err(Error::lua)?;
    let program = SHIM_PROGRAM
        .as_ref()
        .map_err(|message| Error::Lua(message.clone()))?;
    let shims: Table = program
        .load(lua)?
        .call((yield_fn, var_snapshot, models))
        .map_err(Error::lua)?;
    let execute: Function = shims.raw_get("execute").map_err(Error::lua)?;
    globals.raw_set("execute", execute).map_err(Error::lua)?;
    let fanout: Function = shims.raw_get("fanout").map_err(Error::lua)?;
    globals.raw_set("fanout", fanout).map_err(Error::lua)?;
    let tool_call: Function = shims.raw_get("tool_call").map_err(Error::lua)?;
    globals
        .raw_set("tool_call", tool_call)
        .map_err(Error::lua)?;
    let wrap_handle: Function = shims.raw_get("wrap_handle").map_err(Error::lua)?;
    lua.set_named_registry_value(WRAP_HANDLE_REGISTRY, wrap_handle)
        .map_err(Error::lua)?;
    globals
        .raw_set("coroutine", Value::Nil)
        .map_err(Error::lua)?;
    Ok(())
}

/// Installs the live H1 shim base: the coroutine standard library for the
/// yield capture, and the shim prelude's `infer`/`wrap_handle` stashed in
/// the registry so each H1 block's fresh live models table can be wrapped
/// by [`shim_live_h1_models`].
///
/// The H1 control stubs are untouched: `execute`/`fanout`/`jump`/
/// `list_from_section` keep raising before anything can yield. H1's live
/// models table does not exist at construction (the capability resolvers
/// install it per block), so the prelude runs with a nil models table and
/// only its captures are taken.
///
/// # Errors
/// Returns [`Error::Lua`] if the coroutine library, the shim chunk, or any
/// install step fails.
pub fn install_live_h1_shim_base(lua: &Lua) -> Result<()> {
    lua.load_std_libs(StdLib::COROUTINE).map_err(Error::lua)?;
    let globals = lua.globals();
    let coroutine: Table = globals.raw_get("coroutine").map_err(Error::lua)?;
    let yield_fn: Function = coroutine.raw_get("yield").map_err(Error::lua)?;
    let var_snapshot = lua
        .create_function(|lua, ()| var_snapshot_table(lua).map_err(mlua::Error::external))
        .map_err(Error::lua)?;
    let program = SHIM_PROGRAM
        .as_ref()
        .map_err(|message| Error::Lua(message.clone()))?;
    let shims: Table = program
        .load(lua)?
        .call((yield_fn, var_snapshot, Value::Nil))
        .map_err(Error::lua)?;
    let wrap_handle: Function = shims.raw_get("wrap_handle").map_err(Error::lua)?;
    lua.set_named_registry_value(WRAP_HANDLE_REGISTRY, wrap_handle)
        .map_err(Error::lua)?;
    let infer: Function = shims.raw_get("infer").map_err(Error::lua)?;
    lua.set_named_registry_value(INFER_REGISTRY, infer)
        .map_err(Error::lua)?;
    globals
        .raw_set("coroutine", Value::Nil)
        .map_err(Error::lua)?;
    Ok(())
}

/// Wraps one live H1 block's freshly installed live models table:
/// `models.infer` becomes the yield shim and the `bind`/`default` returns
/// become shim-wrapped handle proxies.
///
/// Reapplied on every H1 coroutine step: the capability resolvers install
/// a fresh live models table per step's scope, so each resume re-wraps the
/// fresh table before the thread runs again.
///
/// # Errors
/// Returns [`Error::Lua`] if the base install never ran on this VM, the
/// live models table is absent, or the wrap chunk fails.
pub fn shim_live_h1_models(lua: &Lua) -> Result<()> {
    let wrap_handle: Function = lua
        .named_registry_value(WRAP_HANDLE_REGISTRY)
        .map_err(Error::lua)?;
    let infer: Function = lua
        .named_registry_value(INFER_REGISTRY)
        .map_err(Error::lua)?;
    let models: Table = lua.globals().raw_get("models").map_err(Error::lua)?;
    let program = H1_SHIM_PROGRAM
        .as_ref()
        .map_err(|message| Error::Lua(message.clone()))?;
    program
        .load(lua)?
        .call::<()>((infer, wrap_handle, models))
        .map_err(Error::lua)?;
    Ok(())
}

/// Wraps one model handle as a shimmed proxy table: field reads pass
/// through to the inner userdata and `infer` is the yield shim.
///
/// Everywhere a handle reaches author code in scheduler mode sees the
/// proxy: the `models.use`/`models.get` returns (wrapped by the prelude
/// itself) and the captured alias globals (wrapped here).
///
/// # Errors
/// Returns [`Error::Lua`] if the shim prelude was never installed on this
/// VM or the wrap fails.
pub(crate) fn wrap_shimmed_handle(lua: &Lua, handle: LuaModelHandle) -> Result<Value> {
    let wrap_handle: Function = lua
        .named_registry_value(WRAP_HANDLE_REGISTRY)
        .map_err(Error::lua)?;
    let userdata = lua.create_userdata(handle).map_err(Error::lua)?;
    wrap_handle.call(userdata).map_err(Error::lua)
}
