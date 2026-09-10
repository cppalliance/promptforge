//! The `messages` namespace: optional pure-Lua message-list builders.
//!
//! `messages.new()` returns a normal numerically indexed table whose
//! chainable `system`/`user`/`assistant`/`tool`/`append` methods live behind
//! its metatable, so the list itself stays a plain array of message records:
//! serde conversion, prose substitution, and the chat protocol's validation
//! consume the records exactly as if the author had written the array by
//! hand. The builders perform no validation of their own; the protocol parse
//! owns the whole message contract. The chainable builders are the one
//! deliberate exception to the methodless-handle rule (A9).
//!
//! The shim is pure Lua with no privileged captures (it never yields), so it
//! installs with the host tables during host injection, ahead of the shared
//! replay. The source is pulled in with `include_str!` so chunk line 1 is
//! file line 1, compiled once through the usual [`LuaProgram`] machinery,
//! and loaded per VM; the `@`-prefixed chunk name renders shim frames as
//! verbatim `file:line:` references, like the coroutine shim chunks.

use std::sync::LazyLock;

use mlua::{Function, Table};

use super::{Error, Lua, LuaProgram, Result};

/// The builders chunk's name: `@`-prefixed so PUC renders it verbatim as a
/// file path, making unexpected shim errors clickable `file:line:`
/// references with no `[string "..."]` wrapper.
const MESSAGES_CHUNK_NAME: &str = "@crates/promptforge-lua/src/messages/__impl_messages.lua";

/// The builders source, embedded verbatim so chunk line 1 is file line 1.
const MESSAGES_SOURCE: &str = include_str!("__impl_messages.lua");

/// The builders program, compiled once and loaded per VM. Compilation of the
/// bundled source fails only on a crate bug, so the payload is the error's
/// display string (the crate `Error` is not `Clone`).
static MESSAGES_PROGRAM: LazyLock<std::result::Result<LuaProgram, String>> = LazyLock::new(|| {
    LuaProgram::compile_internal(MESSAGES_SOURCE, MESSAGES_CHUNK_NAME)
        .map_err(|error| error.to_string())
});

/// Installs the `messages` global carrying the pure-Lua `new` builder.
///
/// # Errors
/// Returns [`Error::Lua`] if the builders chunk or the global install fails.
pub(crate) fn install_messages(lua: &Lua, globals: &Table) -> Result<()> {
    let program = MESSAGES_PROGRAM
        .as_ref()
        .map_err(|message| Error::Lua(message.clone()))?;
    let new: Function = program.load(lua)?.call(()).map_err(Error::lua)?;
    let messages = lua.create_table().map_err(Error::lua)?;
    messages.raw_set("new", new).map_err(Error::lua)?;
    globals.raw_set("messages", messages).map_err(Error::lua)
}

#[cfg(test)]
mod tests;
