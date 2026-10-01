//! The `_G` guard: the one metatable on a section VM's globals table.
//!
//! The guard owns the two Engine-owned globals, and no metatable author code
//! sets on `_G` ever sees either key. `argv` outside H1 ([`crate::argv`])
//! is never a raw global: reads return the frozen value and writes raise
//! the freeze refusal. In the H1 pass it behaves as a plain global, so the
//! repair always lands in `_G`. `prose`, each block's lazily rendered text
//! ([`crate::prose`]), is never a raw global either: reads render through
//! the Engine and writes raise the read-only refusal. Every other global
//! read or write goes to the author metatable's `__index` or `__newindex`,
//! looked up live on each access, and otherwise behaves as on a table with
//! no metatable.
//!
//! The guard carries `__metatable`, and the VM's `setmetatable` and
//! `getmetatable` globals are replacements that treat the globals table,
//! compared by raw identity, as the one special case:
//! `setmetatable(_G, mt)` records `mt` as the author's metatable behind the
//! guard, copying its other fields onto the guard, and `getmetatable(_G)`
//! returns the author's metatable (or its `__metatable` field), never the
//! guard. For every other value both behave as the base functions,
//! argument errors and `__metatable` protection included. With no debug
//! library, `rawset`, or code loading in the sandbox, author code has no
//! route to the guard, to a raw `prose` global, or to a raw `argv` global
//! outside H1.
//!
//! The Lua side lives in `__impl_globals.lua`; the Rust side is the slot
//! table the chunk reads, held in the registry.
//!
//! [`RESERVED_NAMES`] is the other half of `_G`'s contract: every name the
//! globals table holds once section setup ends, before any capability
//! prelude installs, plus the Lua keywords. A frontmatter tool alias or
//! model role label installs as a global of its own name, so the parser
//! refuses one that is reserved, and a prelude global may not take one
//! either.

use std::fmt;
use std::sync::LazyLock;

use mlua::{Function, Table};

use super::{Error, Lua, LuaProgram, Result, SharedSource, Value};

/// The guard chunk's name, `@`-prefixed so PUC renders it verbatim as a
/// file path, as the shim chunks' names are.
const GLOBALS_CHUNK_NAME: &str = "@crates/promptforge-internal/lua/src/__impl_globals.lua";

/// The guard source, embedded verbatim so chunk line 1 is file line 1.
const GLOBALS_SOURCE: &str = include_str!("__impl_globals.lua");

/// The registry key of the slot table the guard chunk reads: `argv_frozen`
/// and `argv`, `prose`, and `author`.
const STATE_REGISTRY: &str = "promptforge.globals.state";

/// The guard program, compiled once and loaded per VM, under the shim
/// programs' failure contract.
static GLOBALS_PROGRAM: LazyLock<std::result::Result<LuaProgram, SharedSource>> =
    LazyLock::new(|| {
        LuaProgram::compile_internal(GLOBALS_SOURCE, GLOBALS_CHUNK_NAME)
            .map_err(crate::detail::shared_source_new)
    });

/// Installs the `_G` guard and the `setmetatable` and `getmetatable`
/// replacements on a fresh VM.
///
/// Must run before hardening removes `rawequal`, `rawget`, and `rawset`,
/// which the chunk captures, and before any other chunk captures the base
/// `setmetatable` or `getmetatable`.
///
/// # Errors
/// Returns [`Error::Lua`] if the chunk cannot load or run, or the guard
/// and the replacements cannot be installed.
pub(crate) fn install(lua: &Lua) -> Result<()> {
    let globals = lua.globals();
    let state = lua.create_table().map_err(Error::lua)?;
    let refuse_argv = refusal(lua, crate::argv::ASSIGNMENT_REFUSAL)?;
    let refuse_prose = refusal(lua, crate::prose::ASSIGNMENT_REFUSAL)?;
    let (guard, replace_metatable, read_metatable): (Table, Function, Function) = GLOBALS_PROGRAM
        .as_ref()
        .map_err(Error::shared)?
        .load(lua)?
        .call((globals.clone(), state.clone(), refuse_argv, refuse_prose))
        .map_err(Error::lua)?;
    lua.set_named_registry_value(STATE_REGISTRY, state)
        .map_err(Error::lua)?;
    globals
        .raw_set("setmetatable", replace_metatable)
        .map_err(Error::lua)?;
    globals
        .raw_set("getmetatable", read_metatable)
        .map_err(Error::lua)?;
    globals.set_metatable(Some(guard)).map_err(Error::lua)
}

/// Freezes `argv` on the guard: reads return `frozen`, and every
/// assignment of the global raises the freeze refusal.
///
/// # Errors
/// Returns [`Error::Lua`] if the guard's slot table is missing or cannot
/// be written.
pub(crate) fn freeze_argv(lua: &Lua, frozen: Value) -> Result<()> {
    let state = state(lua)?;
    state.raw_set("argv", frozen).map_err(Error::lua)?;
    state.raw_set("argv_frozen", true).map_err(Error::lua)
}

/// Installs `render` as the source of `prose` reads, replacing the
/// previous block's.
///
/// # Errors
/// Returns [`Error::Lua`] if the guard's slot table is missing or cannot
/// be written.
pub(crate) fn install_prose(lua: &Lua, render: Function) -> Result<()> {
    state(lua)?.raw_set("prose", render).map_err(Error::lua)
}

/// The guard's slot table.
fn state(lua: &Lua) -> Result<Table> {
    lua.named_registry_value(STATE_REGISTRY).map_err(Error::lua)
}

/// An Engine function that raises `message` as a runtime error.
fn refusal(lua: &Lua, message: &'static str) -> Result<Function> {
    lua.create_function(move |_, ()| -> mlua::Result<()> { Err(mlua::Error::runtime(message)) })
        .map_err(Error::lua)
}

/// Why a name is reserved in a section VM's global namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reserved {
    /// A global the Engine installs: on every section VM, or only on some
    /// (`ui` with a Host-state snapshot, `item` in a spawned chain).
    HostGlobal,
    /// A Lua standard-library global the sandbox keeps.
    LuaGlobal,
    /// A Lua 5.5 keyword.
    LuaKeyword,
}

impl fmt::Display for Reserved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Reserved::HostGlobal => "a host global",
            Reserved::LuaGlobal => "a Lua standard-library global",
            Reserved::LuaKeyword => "a Lua keyword",
        })
    }
}

/// Every name reserved in a section VM's global namespace, sorted within
/// each kind: the globals a section or H1 VM holds once section setup
/// ends, before any capability prelude installs, and the Lua 5.5 keywords.
///
/// A global section setup installs must be listed here: the Engine's
/// section setup tests compare this list against a set-up VM's globals in
/// both directions.
pub const RESERVED_NAMES: [(&str, Reserved); 60] = [
    ("args", Reserved::HostGlobal),
    ("argv", Reserved::HostGlobal),
    ("call", Reserved::HostGlobal),
    ("compactors", Reserved::HostGlobal),
    ("fanout", Reserved::HostGlobal),
    ("item", Reserved::HostGlobal),
    ("jump", Reserved::HostGlobal),
    ("list_from_section", Reserved::HostGlobal),
    ("log", Reserved::HostGlobal),
    ("messages", Reserved::HostGlobal),
    ("models", Reserved::HostGlobal),
    ("prose", Reserved::HostGlobal),
    ("store", Reserved::HostGlobal),
    ("sys", Reserved::HostGlobal),
    ("tasks", Reserved::HostGlobal),
    ("tools", Reserved::HostGlobal),
    ("ui", Reserved::HostGlobal),
    ("untrusted", Reserved::HostGlobal),
    ("var", Reserved::HostGlobal),
    ("_G", Reserved::LuaGlobal),
    ("_VERSION", Reserved::LuaGlobal),
    ("assert", Reserved::LuaGlobal),
    ("error", Reserved::LuaGlobal),
    ("getmetatable", Reserved::LuaGlobal),
    ("ipairs", Reserved::LuaGlobal),
    ("math", Reserved::LuaGlobal),
    ("next", Reserved::LuaGlobal),
    ("pairs", Reserved::LuaGlobal),
    ("pcall", Reserved::LuaGlobal),
    ("select", Reserved::LuaGlobal),
    ("setmetatable", Reserved::LuaGlobal),
    ("string", Reserved::LuaGlobal),
    ("table", Reserved::LuaGlobal),
    ("tonumber", Reserved::LuaGlobal),
    ("tostring", Reserved::LuaGlobal),
    ("type", Reserved::LuaGlobal),
    ("xpcall", Reserved::LuaGlobal),
    ("and", Reserved::LuaKeyword),
    ("break", Reserved::LuaKeyword),
    ("do", Reserved::LuaKeyword),
    ("else", Reserved::LuaKeyword),
    ("elseif", Reserved::LuaKeyword),
    ("end", Reserved::LuaKeyword),
    ("false", Reserved::LuaKeyword),
    ("for", Reserved::LuaKeyword),
    ("function", Reserved::LuaKeyword),
    // The vendored Lua 5.5 builds with `LUA_COMPAT_GLOBAL`, so its lexer
    // reads `global` as a contextual keyword rather than a reserved word;
    // the language reserves it all the same.
    ("global", Reserved::LuaKeyword),
    ("goto", Reserved::LuaKeyword),
    ("if", Reserved::LuaKeyword),
    ("in", Reserved::LuaKeyword),
    ("local", Reserved::LuaKeyword),
    ("nil", Reserved::LuaKeyword),
    ("not", Reserved::LuaKeyword),
    ("or", Reserved::LuaKeyword),
    ("repeat", Reserved::LuaKeyword),
    ("return", Reserved::LuaKeyword),
    ("then", Reserved::LuaKeyword),
    ("true", Reserved::LuaKeyword),
    ("until", Reserved::LuaKeyword),
    ("while", Reserved::LuaKeyword),
];

/// Returns why `name` is reserved in a section VM's global namespace, or
/// `None` when a frontmatter alias or a capability prelude global may take
/// it. The match is case-sensitive, as Lua names are.
#[must_use]
pub fn reserved_name(name: &str) -> Option<Reserved> {
    RESERVED_NAMES
        .iter()
        .find(|(reserved, _)| *reserved == name)
        .map(|(_, kind)| *kind)
}

#[cfg(test)]
#[path = "globals-tests.rs"]
mod tests;
