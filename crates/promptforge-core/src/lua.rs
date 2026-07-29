//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available, `args` (the single raw input string) is exposed,
//! and an instruction-count hook aborts a runaway block. The chunk's top-level
//! return value becomes the section's result (the finish case of the exit
//! rule); a chunk that returns nothing yields `None`.

use mlua::{HookTriggers, Lua, LuaOptions, MultiValue, StdLib, Value, VmState};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Error, Result};

/// How many instructions between hook firings.
const HOOK_INTERVAL: u32 = 10_000;
/// Maximum number of hook firings before a block is aborted (~1e7 instructions).
const HOOK_BUDGET: u64 = 1_000;

/// Run a section's Lua chunk with `args` exposed, returning its top-level return
/// value as a string, or `None` if the chunk returned nothing.
///
/// # Errors
/// Returns [`Error::Lua`] if the sandbox cannot be built, the chunk fails to run
/// (including hitting the instruction budget), or it returns a value that cannot
/// be rendered as a result string.
pub fn run_chunk(source: &str, args: &str) -> Result<Option<String>> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH,
        LuaOptions::default(),
    )
    .map_err(|e| Error::Lua(e.to_string()))?;

    harden(&lua)?;
    lua.globals()
        .set("args", args)
        .map_err(|e| Error::Lua(e.to_string()))?;
    install_instruction_budget(&lua);

    let returned: MultiValue = lua
        .load(source)
        .eval()
        .map_err(|e| Error::Lua(e.to_string()))?;

    match returned.into_iter().next() {
        None | Some(Value::Nil) => Ok(None),
        Some(value) => Ok(Some(value_to_string(&value)?)),
    }
}

/// Remove code-loading and reflection globals the base library provides. The
/// `io`, `os`, `package`, `coroutine`, and `debug` libraries are never loaded.
fn harden(lua: &Lua) -> Result<()> {
    let globals = lua.globals();
    for name in [
        "load",
        "loadstring",
        "dofile",
        "loadfile",
        "collectgarbage",
        "require",
        "getfenv",
        "setfenv",
        "rawget",
        "rawset",
        "rawequal",
        "rawlen",
    ] {
        globals
            .set(name, Value::Nil)
            .map_err(|e| Error::Lua(e.to_string()))?;
    }
    Ok(())
}

/// Install an instruction-count hook that aborts a runaway block.
fn install_instruction_budget(lua: &Lua) {
    let fired = Arc::new(AtomicU64::new(0));
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_lua, _debug| {
            if fired.fetch_add(1, Ordering::Relaxed) >= HOOK_BUDGET {
                return Err(mlua::Error::RuntimeError(
                    "lua instruction budget exceeded".to_string(),
                ));
            }
            Ok(VmState::Continue)
        },
    );
}

/// Render a returned Lua scalar as the section's result string. Tables and other
/// non-scalar returns are deferred to a later commit.
fn value_to_string(value: &Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.to_string_lossy()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        other => Err(Error::Lua(format!(
            "cannot return a {} as a result",
            other.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_args_verbatim() {
        assert_eq!(
            run_chunk("return args", "hello").unwrap().as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn no_return_is_none() {
        assert_eq!(run_chunk("local x = 1", "hello").unwrap(), None);
    }

    #[test]
    fn safe_stdlib_present() {
        // string/table/math and base tostring are available.
        let out = run_chunk("return tostring(#args) .. ',' .. string.upper(args)", "hi").unwrap();
        assert_eq!(out.as_deref(), Some("2,HI"));
    }

    #[test]
    fn dangerous_globals_absent() {
        let out = run_chunk(
            "return tostring(io) .. ',' .. tostring(os) .. ',' .. tostring(require) .. ',' .. tostring(load)",
            "",
        )
        .unwrap();
        assert_eq!(out.as_deref(), Some("nil,nil,nil,nil"));
    }

    #[test]
    fn instruction_budget_aborts_runaway() {
        assert!(run_chunk("while true do end", "").is_err());
    }
}
