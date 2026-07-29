//! Sandboxed Lua execution for a section's Lua block.
//!
//! A section's Lua chunk runs in a fresh, restricted `mlua` VM: only the
//! `string`, `table`, and `math` standard libraries plus the safe base
//! functions are available; the raw input `args` string and the runtime `sys`
//! table are exposed; a writable `var` table is provided for the block to
//! populate; and an instruction-count hook aborts a runaway block.
//!
//! The chunk's top-level return value becomes the section's result (the finish
//! case of the exit rule). The `var` table is read back afterward as JSON for
//! prose substitution.

use mlua::{
    HookTriggers, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Value, Variadic, VmState,
};
use serde_json::Value as Json;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Error, Result};

/// How many instructions between hook firings.
const HOOK_INTERVAL: u32 = 10_000;
/// Maximum number of hook firings before a block is aborted (~1e7 instructions).
const HOOK_BUDGET: u64 = 1_000;

/// The result of running a section's Lua block.
#[derive(Debug, Clone)]
pub struct LuaOutcome {
    /// The chunk's top-level return value, if it returned one (the finish case).
    pub returned: Option<String>,
    /// The `var` table after the block ran, as JSON, for prose substitution.
    pub var: Json,
    /// The tool names the block advertised with `tools.add(...)`, in first-seen
    /// order and de-duplicated. Empty when the block never called `tools.add`.
    /// These names are recorded verbatim and are not validated here against any
    /// tool registry; the executor resolves them per section.
    pub scoped_tools: Vec<String>,
}

/// Run a section's Lua chunk with `args` and `sys` exposed and a writable `var`
/// table available, returning the chunk's return value and the final `var`.
///
/// # Errors
/// Returns [`Error::Lua`] if the sandbox cannot be built, `sys`/`var` cannot be
/// bridged, the chunk fails to run (including hitting the instruction budget),
/// or it returns a value that cannot be rendered as a result string.
pub fn run_chunk(source: &str, args: &str, sys: &Json) -> Result<LuaOutcome> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH,
        LuaOptions::default(),
    )
    .map_err(|e| Error::Lua(e.to_string()))?;

    harden(&lua)?;

    let globals = lua.globals();
    globals
        .set("args", args)
        .map_err(|e| Error::Lua(e.to_string()))?;
    let sys_value = lua.to_value(sys).map_err(|e| Error::Lua(e.to_string()))?;
    globals
        .set("sys", sys_value)
        .map_err(|e| Error::Lua(e.to_string()))?;
    let var_table = lua.create_table().map_err(|e| Error::Lua(e.to_string()))?;
    globals
        .set("var", var_table)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let scoped = install_tools_table(&lua, &globals)?;

    install_instruction_budget(&lua);

    let returned: MultiValue = lua
        .load(source)
        .eval()
        .map_err(|e| Error::Lua(e.to_string()))?;
    let returned = match returned.into_iter().next() {
        None | Some(Value::Nil) => None,
        Some(value) => Some(value_to_string(&value)?),
    };

    let var_value: Value = globals.get("var").map_err(|e| Error::Lua(e.to_string()))?;
    let var: Json = lua
        .from_value(var_value)
        .map_err(|e| Error::Lua(e.to_string()))?;

    let scoped_tools = scoped.borrow().clone();

    Ok(LuaOutcome {
        returned,
        var,
        scoped_tools,
    })
}

/// Expose a `tools` table whose `add` host function records tool names into a
/// shared, ordered, de-duplicated collection, and return a handle to that
/// collection so the caller can read the accumulated names back after the
/// chunk runs. `add` only records names; it validates nothing and never
/// touches the model.
///
/// The VM is single-threaded and non-async, so an `Rc<RefCell<..>>` moved into
/// the function is sufficient shared state.
///
/// # Errors
/// Returns [`Error::Lua`] if the `tools` table or its `add` function cannot be
/// created or installed into the sandbox globals.
fn install_tools_table(lua: &Lua, globals: &mlua::Table) -> Result<Rc<RefCell<Vec<String>>>> {
    let scoped: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = Rc::clone(&scoped);
    let add = lua
        .create_function(move |_, names: Variadic<String>| {
            let mut acc = recorder.borrow_mut();
            for name in names {
                if !acc.iter().any(|existing| existing == &name) {
                    acc.push(name);
                }
            }
            Ok(())
        })
        .map_err(|e| Error::Lua(e.to_string()))?;
    let tools = lua.create_table().map_err(|e| Error::Lua(e.to_string()))?;
    tools
        .set("add", add)
        .map_err(|e| Error::Lua(e.to_string()))?;
    globals
        .set("tools", tools)
        .map_err(|e| Error::Lua(e.to_string()))?;
    Ok(scoped)
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
    use serde_json::json;

    fn run(source: &str, args: &str) -> Result<LuaOutcome> {
        run_chunk(source, args, &json!({ "id": 1, "when": "t" }))
    }

    #[test]
    fn returns_args_verbatim() {
        assert_eq!(
            run("return args", "hello").unwrap().returned.as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn no_return_is_none() {
        assert_eq!(run("local x = 1", "hello").unwrap().returned, None);
    }

    #[test]
    fn reads_sys() {
        assert_eq!(
            run("return sys.id", "").unwrap().returned.as_deref(),
            Some("1")
        );
    }

    #[test]
    fn var_is_read_back() {
        let out = run("var.greeting = 'hi ' .. args", "bob").unwrap();
        assert_eq!(
            out.var.get("greeting").and_then(|v| v.as_str()),
            Some("hi bob")
        );
    }

    #[test]
    fn safe_stdlib_present() {
        let out = run("return string.upper(args)", "hi").unwrap();
        assert_eq!(out.returned.as_deref(), Some("HI"));
    }

    #[test]
    fn dangerous_globals_absent() {
        let out = run(
            "return tostring(io) .. ',' .. tostring(os) .. ',' .. tostring(require) .. ',' .. tostring(load)",
            "",
        )
        .unwrap();
        assert_eq!(out.returned.as_deref(), Some("nil,nil,nil,nil"));
    }

    #[test]
    fn instruction_budget_aborts_runaway() {
        assert!(run("while true do end", "").is_err());
    }

    #[test]
    fn single_add_records_its_names() {
        let out = run("tools.add('web_search', 'web_fetch')", "").unwrap();
        assert_eq!(out.scoped_tools, vec!["web_search", "web_fetch"]);
    }

    #[test]
    fn multiple_adds_accumulate_and_dedupe() {
        let out = run(
            "tools.add('web_search')\ntools.add('web_fetch', 'web_search')",
            "",
        )
        .unwrap();
        assert_eq!(out.scoped_tools, vec!["web_search", "web_fetch"]);
    }

    #[test]
    fn add_inside_if_branch_records() {
        let out = run("if true then tools.add('web_search') end", "").unwrap();
        assert_eq!(out.scoped_tools, vec!["web_search"]);
    }

    #[test]
    fn no_add_leaves_scoped_tools_empty() {
        let out = run("local x = 1", "").unwrap();
        assert!(out.scoped_tools.is_empty());
    }
}
