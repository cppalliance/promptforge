//! The coroutine-protocol shim layer: per-VM Lua yield wrappers for the
//! suspending host calls.
//!
//! Yield cannot cross the C boundary, so `models.infer`, `call`, `fanout`,
//! and `tools.call` are Lua shims (source in `__impl_coro.lua` beside this
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

use super::{Error, Lua, LuaProgram, Result, SharedSource, StdLib, var_snapshot_table};

/// The shim chunk's name: `@`-prefixed so PUC renders it verbatim as a file
/// path, making unexpected shim errors clickable `file:line:` references.
const SHIM_CHUNK_NAME: &str = "@crates/promptforge-core/src/lua/__impl_coro.lua";

/// The shim source, embedded verbatim so chunk line 1 is file line 1.
const SHIM_SOURCE: &str = include_str!("__impl_coro.lua");

/// The registry key for the shim's `chat`, stashed by the prelude install so
/// an agent host can install it as `models.chat`. The registry is host-side
/// only: a section VM's `models.chat` stays nil because nothing ever reads
/// this stash there.
const CHAT_REGISTRY: &str = "promptforge.impl_coro.chat";

/// The registry key for the shim's `infer`, stashed by the live H1 base
/// install so each H1 block's fresh live models table can receive it.
const INFER_REGISTRY: &str = "promptforge.impl_coro.infer";

/// The registry key for the shim's `loop`, stashed by the prelude install so
/// a section VM's host can install it as `models.loop`. The registry is
/// host-side only: an agent VM's `models.loop` stays nil because nothing
/// ever reads this stash there.
const LOOP_REGISTRY: &str = "promptforge.impl_coro.loop";

/// The registry key for the shim's `user_input`, stashed by the prelude
/// install so a section VM's host can install it as the `user_input`
/// global. The registry is host-side only: an agent VM's `user_input`
/// stays nil because nothing ever reads this stash there.
const USER_INPUT_REGISTRY: &str = "promptforge.impl_coro.user_input";

/// The registry key for the shim's store function table, stashed by the
/// prelude and the live H1 base install so the executor can install the
/// store yield shims onto a VM's `store` table. The registry is host-side
/// only: an agent VM never installs them, so its store table keeps the
/// direct closures - the agent driver is a single-identity loop with no
/// interleaving for the claims model to govern.
const STORE_REGISTRY: &str = "promptforge.impl_coro.store";

/// The shim program, compiled once and loaded per VM. Compilation of the
/// bundled source fails only on a crate bug, so the payload is a shareable
/// [`SharedSource`] cause (the crate `Error` is not `Clone`), re-wrapped as
/// a typed error at each install.
static SHIM_PROGRAM: LazyLock<std::result::Result<LuaProgram, SharedSource>> =
    LazyLock::new(|| {
        LuaProgram::compile_internal(SHIM_SOURCE, SHIM_CHUNK_NAME).map_err(SharedSource::new)
    });

/// Installs the yield shims on a VM whose host tables already exist.
///
/// Scheduler-mode VMs load the coroutine standard library for the shim's
/// `yield` capture (legacy VMs keep exactly `STRING | TABLE | MATH`); the
/// `coroutine` global is stripped again before returning, so author code
/// cannot yield directly and a hand-rolled yield fails the driver's strict
/// validation. The `models` and `tools` tables are passed to the shim chunk
/// as arguments, so the chunk never reads a global; the chunk shims
/// `models.infer` and installs `tools.call`, and the `call`/`fanout` shims
/// come back for the host to install. The `models.loop` shim is stashed in
/// the registry for [`install_section_loop_shim`], so agent VMs - which run
/// this prelude too - never receive it.
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
    let tools: Table = globals.raw_get("tools").map_err(Error::lua)?;
    let program = SHIM_PROGRAM.as_ref().map_err(Error::shared)?;
    let shims: Table = program
        .load(lua)?
        .call((yield_fn, var_snapshot, models, tools))
        .map_err(Error::lua)?;
    let call: Function = shims.raw_get("call").map_err(Error::lua)?;
    globals.raw_set("call", call).map_err(Error::lua)?;
    let fanout: Function = shims.raw_get("fanout").map_err(Error::lua)?;
    globals.raw_set("fanout", fanout).map_err(Error::lua)?;
    let chat: Function = shims.raw_get("chat").map_err(Error::lua)?;
    lua.set_named_registry_value(CHAT_REGISTRY, chat)
        .map_err(Error::lua)?;
    let models_loop: Function = shims.raw_get("loop").map_err(Error::lua)?;
    lua.set_named_registry_value(LOOP_REGISTRY, models_loop)
        .map_err(Error::lua)?;
    let user_input: Function = shims.raw_get("user_input").map_err(Error::lua)?;
    lua.set_named_registry_value(USER_INPUT_REGISTRY, user_input)
        .map_err(Error::lua)?;
    let store: Table = shims.raw_get("store").map_err(Error::lua)?;
    lua.set_named_registry_value(STORE_REGISTRY, store)
        .map_err(Error::lua)?;
    globals
        .raw_set("coroutine", Value::Nil)
        .map_err(Error::lua)?;
    Ok(())
}

/// Installs the section-only `models.loop` yield shim on a VM whose shim
/// prelude already ran (`install_shim_prelude` stashed the shim in the
/// registry).
///
/// The executor's section setup is the only caller: `models.loop` never
/// exists in an agent VM - not stubbed, simply absent - so an agent program
/// calling it fails as an undefined global, the mirror of the agent-only
/// `models.chat`.
///
/// # Errors
/// Returns [`Error::Lua`] if the shim prelude was never installed on this
/// VM, the `models` table is absent, or the install fails.
pub fn install_section_loop_shim(lua: &Lua) -> Result<()> {
    let models_loop: Function = lua
        .named_registry_value(LOOP_REGISTRY)
        .map_err(Error::lua)?;
    let models: Table = lua.globals().raw_get("models").map_err(Error::lua)?;
    models.raw_set("loop", models_loop).map_err(Error::lua)
}

/// Installs the section-only `user_input` yield shim as a global on a VM
/// whose shim prelude already ran (`install_shim_prelude` stashed the shim
/// in the registry).
///
/// The executor's section setup is the only caller: `user_input` never
/// exists in an agent VM - not stubbed, simply absent - so an agent
/// program calling it fails as an undefined global, the mirror of the
/// agent-only `models.chat`.
///
/// # Errors
/// Returns [`Error::Lua`] if the shim prelude was never installed on this
/// VM or the install fails.
pub fn install_section_user_input_shim(lua: &Lua) -> Result<()> {
    let user_input: Function = lua
        .named_registry_value(USER_INPUT_REGISTRY)
        .map_err(Error::lua)?;
    lua.globals()
        .raw_set("user_input", user_input)
        .map_err(Error::lua)
}

/// Installs the agent-only `models.chat` yield shim on a VM whose shim
/// prelude already ran (`install_shim_prelude` stashed the shim in the
/// registry).
///
/// The agent executor is the only caller: `models.chat` never exists in a
/// section VM - not stubbed, simply absent - so a document prompt calling
/// it fails as an undefined global.
///
/// # Errors
/// Returns [`Error::Lua`] if the shim prelude was never installed on this
/// VM, the `models` table is absent, or the install fails.
pub fn install_agent_chat_shim(lua: &Lua) -> Result<()> {
    let chat: Function = lua
        .named_registry_value(CHAT_REGISTRY)
        .map_err(Error::lua)?;
    let models: Table = lua.globals().raw_get("models").map_err(Error::lua)?;
    models.raw_set("chat", chat).map_err(Error::lua)
}

/// Installs the live H1 shim base: the coroutine standard library for the
/// yield capture, and the shim prelude's `infer` stashed in the registry so
/// each H1 block's fresh live models table can receive it through
/// [`shim_live_h1_models`].
///
/// The H1 control stubs are untouched: `call`/`fanout`/`jump`/
/// `list_from_section` keep raising before anything can yield. H1's live
/// models table does not exist at construction (the capability resolvers
/// install it per block), so the prelude runs with nil namespace tables and
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
    let program = SHIM_PROGRAM.as_ref().map_err(Error::shared)?;
    let shims: Table = program
        .load(lua)?
        .call((yield_fn, var_snapshot, Value::Nil, Value::Nil))
        .map_err(Error::lua)?;
    let infer: Function = shims.raw_get("infer").map_err(Error::lua)?;
    lua.set_named_registry_value(INFER_REGISTRY, infer)
        .map_err(Error::lua)?;
    let store: Table = shims.raw_get("store").map_err(Error::lua)?;
    lua.set_named_registry_value(STORE_REGISTRY, store)
        .map_err(Error::lua)?;
    globals
        .raw_set("coroutine", Value::Nil)
        .map_err(Error::lua)?;
    Ok(())
}

/// Installs the store yield shims onto a VM's `store` table, replacing the
/// direct closures the host API install put there. Every store operation
/// then suspends the block as a leaf yield the driver answers against the
/// sync VFS via the blocking pool - uniformly for all backends, with no
/// inline fast path, so interleaving behavior never depends on which
/// backend serves the mount.
///
/// The executor's section setup and live H1 setup are the only callers:
/// an agent VM never receives the shims (its driver is a single-identity
/// loop with no interleaving for the claims model to govern), so its
/// store table keeps the direct closures.
///
/// # Errors
/// Returns [`Error::Lua`] if the shim prelude (or the live H1 base
/// install) never ran on this VM, the `store` table is absent, or the
/// install fails.
pub fn install_store_shims(lua: &Lua) -> Result<()> {
    let shims: Table = lua
        .named_registry_value(STORE_REGISTRY)
        .map_err(Error::lua)?;
    let store: Table = lua.globals().raw_get("store").map_err(Error::lua)?;
    for pair in shims.pairs::<String, Function>() {
        let (name, function) = pair.map_err(Error::lua)?;
        store.raw_set(name, function).map_err(Error::lua)?;
    }
    Ok(())
}

/// Gives one live H1 block's freshly installed live models table the yield
/// shim as its `models.infer`.
///
/// Reapplied on every H1 coroutine step: the capability resolvers install
/// a fresh live models table per step's scope, so each resume re-installs
/// the shim on the fresh table before the thread runs again. The handles
/// `models.bind`/`models.default` return are plain userdata: invocation is
/// namespace-only, `models.infer(handle?, prompt)`.
///
/// # Errors
/// Returns [`Error::Lua`] if the base install never ran on this VM or the
/// live models table is absent.
pub fn shim_live_h1_models(lua: &Lua) -> Result<()> {
    let infer: Function = lua
        .named_registry_value(INFER_REGISTRY)
        .map_err(Error::lua)?;
    let models: Table = lua.globals().raw_get("models").map_err(Error::lua)?;
    models.raw_set("infer", infer).map_err(Error::lua)
}
