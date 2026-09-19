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
use crate::error_value::{Raised, install_error_value, install_normalize_failure, raised_from};

/// The shim chunk's name: `@`-prefixed so PUC renders it verbatim as a file
/// path, making unexpected shim errors clickable `file:line:` references.
const SHIM_CHUNK_NAME: &str = "@crates/promptforge-api-runtime/src/lua/__impl_coro.lua";

/// The shim source, embedded verbatim so chunk line 1 is file line 1.
const SHIM_SOURCE: &str = include_str!("__impl_coro.lua");

/// The registry key for the shim's `chat`, stashed by the prelude install so
/// an agent host can install it as `models.chat`. The registry is host-side
/// only: a section VM's `models.chat` stays nil because nothing ever reads
/// this stash there.
const CHAT_REGISTRY: &str = "promptforge.impl_coro.chat";

/// The registry key for the shim's `loop`, stashed by the prelude install so
/// a section VM's host can install it as `models.loop`. The registry is
/// host-side only: an agent VM's `models.loop` stays nil because nothing
/// ever reads this stash there.
const LOOP_REGISTRY: &str = "promptforge.impl_coro.loop";

/// The registry key for the shim's model-issued `tool_call` form, stashed
/// by the prelude install so a test host can install it as
/// `tools.call_as_model` and drive the driver's `call_id` path from a
/// fixture section. The registry is host-side only: in production the
/// loop shim reaches the function directly inside the prelude chunk, and
/// no VM ever installs it as a global.
const MODEL_TOOL_CALL_REGISTRY: &str = "promptforge.impl_coro.model_tool_call";

/// The registry key for the shim's `user_input`, stashed by the prelude
/// install so a section VM's host can install it as the `user_input`
/// global. The registry is host-side only: an agent VM's `user_input`
/// stays nil because nothing ever reads this stash there.
const USER_INPUT_REGISTRY: &str = "promptforge.impl_coro.user_input";

/// The registry key for the shim's store function table, stashed by the
/// prelude install so the executor can install the store yield shims onto a
/// VM's `store` table. The registry is host-side
/// only: an agent VM never installs them, so its store table keeps the
/// direct closures - the agent driver is a single-identity loop with no
/// interleaving for the claims model to govern.
const STORE_REGISTRY: &str = "promptforge.impl_coro.store";

/// The registry key for the shim's block guard, stashed by the prelude
/// install so [`block_guard`] can wrap every block coroutine the VM starts.
const GUARD_REGISTRY: &str = "promptforge.impl_coro.guard";

/// The registry key of the last value a guarded block raised, written by
/// the guard's `stash_failure` capture from the message handler at the
/// raise point and taken by [`take_failure`] when the failure reaches the
/// host.
const FAILURE_REGISTRY: &str = "promptforge.impl_coro.failure";

/// The registry key of the traceback recorded beside the stashed failure:
/// the coroutine's stack at the raise point, before the guard's `xpcall`
/// unwinds the block's frames. The guard's re-raise happens after that
/// unwinding, so the traceback mlua appends to the re-raised error shows
/// only the guard's own frame; this one carries the author's.
const FAILURE_TRACEBACK_REGISTRY: &str = "promptforge.impl_coro.failure_traceback";

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
/// validation. The `models`, `tools`, and `compactors` tables are passed
/// to the shim chunk as arguments, so the chunk never reads a global; the
/// chunk shims `models.infer` and installs `tools.call`, and the
/// `call`/`fanout` shims and the `tasks` namespace table come back for the
/// host to install as globals. The
/// `models.loop` shim is stashed in the registry for
/// [`install_section_loop_shim`], so agent VMs - which run this prelude
/// too - never receive it. `max_tool_iterations` is the loop's round cap,
/// the run's resolved value, captured by the chunk so the shim needs no
/// host call to read it.
///
/// Three further captures give the chunk the structured error shape:
/// `error_value(kind, fields)` builds the `{ kind, message, ... }` table
/// every failure takes on its way to author code, `stash_failure` lets
/// the block guard record a raised value for [`take_failure`] before mlua
/// stringifies it, and `normalize_failure` rewrites a Rust callback's
/// raised failure into the same table. The chunk's `pcall` and `xpcall`
/// replacements, which run every caught value through that capture, are
/// installed over the base library's globals here, so a host callback that
/// fails directly from Rust reaches author code in the one shape.
///
/// # Errors
/// Returns [`Error::Lua`] if the coroutine library, the shim chunk, or any
/// install step fails.
pub(crate) fn install_shim_prelude(lua: &Lua, max_tool_iterations: usize) -> Result<()> {
    lua.load_std_libs(StdLib::COROUTINE).map_err(Error::lua)?;
    let globals = lua.globals();
    let coroutine: Table = globals.raw_get("coroutine").map_err(Error::lua)?;
    let yield_fn: Function = coroutine.raw_get("yield").map_err(Error::lua)?;
    let var_snapshot = lua
        .create_function(|lua, ()| var_snapshot_table(lua).map_err(mlua::Error::external))
        .map_err(Error::lua)?;
    let models: Table = globals.raw_get("models").map_err(Error::lua)?;
    let tools: Table = globals.raw_get("tools").map_err(Error::lua)?;
    let compactors: Table = globals.raw_get("compactors").map_err(Error::lua)?;
    let error_value = install_error_value(lua).map_err(Error::lua)?;
    let stash_failure = lua
        .create_function(|lua, failure: Value| {
            // Called from the guard's message handler, so the failing
            // frames are still on this coroutine's stack: level 1 starts
            // the traceback at the handler, above this capture's own frame.
            let traceback = lua.traceback(None, 1)?;
            lua.set_named_registry_value(FAILURE_TRACEBACK_REGISTRY, traceback)?;
            lua.set_named_registry_value(FAILURE_REGISTRY, failure)
        })
        .map_err(Error::lua)?;
    let normalize_failure = install_normalize_failure(lua).map_err(Error::lua)?;
    let program = SHIM_PROGRAM.as_ref().map_err(Error::shared)?;
    let shims: Table = program
        .load(lua)?
        .call((
            yield_fn,
            var_snapshot,
            models,
            tools,
            compactors,
            max_tool_iterations,
            error_value,
            stash_failure,
            normalize_failure,
        ))
        .map_err(Error::lua)?;
    let guard: Function = shims.raw_get("guard").map_err(Error::lua)?;
    lua.set_named_registry_value(GUARD_REGISTRY, guard)
        .map_err(Error::lua)?;
    for name in ["pcall", "xpcall"] {
        let protected: Function = shims.raw_get(name).map_err(Error::lua)?;
        globals.raw_set(name, protected).map_err(Error::lua)?;
    }
    let call: Function = shims.raw_get("call").map_err(Error::lua)?;
    globals.raw_set("call", call).map_err(Error::lua)?;
    let tasks: Table = shims.raw_get("tasks").map_err(Error::lua)?;
    globals.raw_set("tasks", tasks).map_err(Error::lua)?;
    let fanout: Function = shims.raw_get("fanout").map_err(Error::lua)?;
    globals.raw_set("fanout", fanout).map_err(Error::lua)?;
    let chat: Function = shims.raw_get("chat").map_err(Error::lua)?;
    lua.set_named_registry_value(CHAT_REGISTRY, chat)
        .map_err(Error::lua)?;
    let models_loop: Function = shims.raw_get("loop").map_err(Error::lua)?;
    lua.set_named_registry_value(LOOP_REGISTRY, models_loop)
        .map_err(Error::lua)?;
    let model_tool_call: Function = shims.raw_get("model_tool_call").map_err(Error::lua)?;
    lua.set_named_registry_value(MODEL_TOOL_CALL_REGISTRY, model_tool_call)
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

/// Returns the shim's block guard for a VM whose shim prelude already ran.
///
/// The host creates every block coroutine from the guard and resumes it
/// first with the block function: the guard runs the block under `xpcall`
/// (yields pass through), stashes a raised value and the raise-point
/// traceback for [`take_failure`] from the message handler, and re-raises
/// the same value, so a shim's structured error table reaches the host
/// intact instead of only as mlua's stringification.
///
/// # Errors
/// Returns [`Error::Lua`] if the shim prelude never ran on this VM.
pub(crate) fn block_guard(lua: &Lua) -> Result<Function> {
    lua.named_registry_value(GUARD_REGISTRY).map_err(Error::lua)
}

/// What the guard stashed for the last failure of a guarded block.
#[derive(Debug, Default)]
pub(crate) struct StashedFailure {
    /// The raised value read back as a [`Raised`] when it is a structured
    /// error table built on this VM; `None` when it was a plain string, an
    /// author's own table, or a Rust callback's wrapped failure.
    pub(crate) raised: Option<Raised>,
    /// The coroutine's traceback at the raise point, `stack traceback:`
    /// heading included, when the handler recorded one.
    pub(crate) traceback: Option<String>,
}

impl StashedFailure {
    /// Restores the raise-point traceback onto a Lua-raised failure.
    ///
    /// mlua appends the coroutine's traceback to a runtime error when the
    /// coroutine dies, but the guard's re-raise is what kills it, so that
    /// traceback shows the guard's frame and nothing of the block's. When
    /// this stash recorded the real one, the appended tail is replaced with
    /// it, so the line mapper sees the author's frames. A Rust callback's
    /// wrapped failure carries its own traceback and is left untouched.
    pub(crate) fn restore_traceback<'e>(
        &self,
        error: &'e mlua::Error,
    ) -> std::borrow::Cow<'e, mlua::Error> {
        let (mlua::Error::RuntimeError(message), Some(traceback)) = (error, &self.traceback) else {
            return std::borrow::Cow::Borrowed(error);
        };
        let head = message
            .rfind("\nstack traceback:")
            .map_or(message.as_str(), |at| &message[..at]);
        std::borrow::Cow::Owned(mlua::Error::RuntimeError(format!("{head}\n{traceback}")))
    }
}

/// Takes what the guard stashed for the last failure of a guarded block.
/// Both slots are cleared on every call, so a later failure never sees a
/// stale value; the default (nothing raised, no traceback) when nothing
/// was stashed.
///
/// # Errors
/// Returns [`Error::Lua`] if a registry slot cannot be read or cleared.
pub(crate) fn take_failure(lua: &Lua) -> Result<StashedFailure> {
    let failure: Value = lua
        .named_registry_value(FAILURE_REGISTRY)
        .map_err(Error::lua)?;
    let traceback: Value = lua
        .named_registry_value(FAILURE_TRACEBACK_REGISTRY)
        .map_err(Error::lua)?;
    if matches!(failure, Value::Nil) {
        return Ok(StashedFailure::default());
    }
    lua.set_named_registry_value(FAILURE_REGISTRY, Value::Nil)
        .map_err(Error::lua)?;
    lua.set_named_registry_value(FAILURE_TRACEBACK_REGISTRY, Value::Nil)
        .map_err(Error::lua)?;
    let traceback = match traceback {
        Value::String(text) => Some(text.to_str().map_err(Error::lua)?.to_owned()),
        _ => None,
    };
    Ok(StashedFailure {
        raised: raised_from(lua, &failure).map_err(Error::lua)?,
        traceback,
    })
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

/// Installs the model-issued `tool_call` form as `tools.call_as_model` on a
/// VM whose shim prelude already ran, so a fixture section can yield a
/// `tool_call` carrying a `call_id` straight at the driver's dispatch arm.
///
/// Test hosts are the only callers, so the install exists only under the
/// `test-support` feature: in production the loop shim reaches the
/// function directly inside the prelude chunk, and `tools.call_as_model`
/// never exists in any VM - not stubbed, simply absent.
///
/// # Errors
/// Returns [`Error::Lua`] if the shim prelude was never installed on this
/// VM, the `tools` table is absent, or the install fails.
#[cfg(feature = "test-support")]
pub fn install_model_tool_call_shim(lua: &Lua) -> Result<()> {
    let model_tool_call: Function = lua
        .named_registry_value(MODEL_TOOL_CALL_REGISTRY)
        .map_err(Error::lua)?;
    let tools: Table = lua.globals().raw_get("tools").map_err(Error::lua)?;
    tools
        .raw_set("call_as_model", model_tool_call)
        .map_err(Error::lua)
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
/// Returns [`Error::Lua`] if the shim prelude never ran on this VM, the
/// `store` table is absent, or the install fails.
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
