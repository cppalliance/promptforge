//! The `tools` namespace: declaration, scoping, invocation, and counts.
//!
//! One Lua table carries every tool operation, mirroring the `models.*`
//! namespacing of model operations: `bind` and `always` declare during
//! live H1 (forbidden stubs here), `add` and `add_local` scope tools into
//! the section, `call` dispatches a bound tool by alias or Tool object
//! (installed by the coroutine shim prelude, since dispatch suspends), and
//! `calls` is the read-only per-alias dispatch counter surface. The
//! installation logic lives here, out of the VM driver; the VM only calls
//! the installers in setup order.

use std::sync::{Arc, Mutex};

use mlua::{Function, Lua, MultiValue, Table, Value, Variadic};
use promptforge_model_client::client::ToolSchema;

use crate::error::{Error, Result};
use crate::handles::{ToolBinding, ToolSet};
use crate::live::validate_alias;
use crate::scope::{ToolCallCounts, ToolRuntime};
use crate::vm::LocalTools;

mod decode;
mod userdata;

pub(crate) use userdata::LuaToolHandle;

pub(crate) use decode::tool_alias;

use decode::{add_local_params_schema, collect_tools_add_entries};

/// Installs the read-only `tools.calls` counter table over `counts`;
/// `declared` feeds the unknown-key diagnostic.
///
/// # Errors
/// Returns [`Error::Lua`] if the Lua table or callbacks cannot be created or
/// installed.
pub(crate) fn install_lua_tool_calls(
    lua: &Lua,
    counts: &ToolCallCounts,
    declared: &[String],
) -> Result<()> {
    let globals = lua.globals();
    let tools: Table = globals.raw_get("tools").map_err(Error::lua)?;

    let calls_inner = lua.create_table().map_err(Error::lua)?;
    let meta = lua.create_table().map_err(Error::lua)?;

    let counts_for_index = counts.clone();
    let declared: Vec<String> = declared.to_vec();
    let index = lua
        .create_function(move |_, (_table, key): (Table, String)| {
            let value = counts_for_index.get(&key).map_err(mlua::Error::external)?;
            if let Some(count) = value {
                Ok(count)
            } else {
                let seeded = counts_for_index.aliases().map_err(mlua::Error::external)?;
                let declared_unseeded = declared.iter().any(|alias| alias == &key);
                Err(mlua::Error::external(format!(
                    "tools.calls: {key:?} has no seeded count; \
                     seeded aliases: {seeded:?}{}",
                    if declared_unseeded {
                        " (alias was declared by tools.bind but neither added to \
                         this section's scope nor dispatched by tools.call)"
                    } else if seeded.is_empty() {
                        ""
                    } else {
                        " - check for typos or add it via tools.add"
                    }
                )))
            }
        })
        .map_err(Error::lua)?;
    meta.set("__index", index).map_err(Error::lua)?;

    let newindex_err = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external("tools.calls is read-only"))
        })
        .map_err(Error::lua)?;
    meta.set("__newindex", newindex_err).map_err(Error::lua)?;

    calls_inner.set_metatable(Some(meta)).map_err(Error::lua)?;

    tools.set("calls", calls_inner).map_err(Error::lua)?;
    Ok(())
}

/// Installs `tools.calls` as a read-only Lua table backed by a fresh
/// [`ToolCallCounts`]. Each seeded alias reads its live count; indexing an
/// unseeded key is a hard error that names the bad key and lists the seeded
/// set. When the key was declared by `tools.bind` but never seeded - neither
/// scoped into the section nor dispatched by a script `tools.call` - the
/// diagnostic says so.
///
/// Returns the `ToolCallCounts` handle so the executor's tool loop can
/// increment it.
///
/// # Errors
/// Returns [`Error::Lua`] when installing the `tools.calls` index fails.
pub(crate) fn install_tool_call_counts(
    lua: &Lua,
    bound_tools: &ToolSet,
    bindings: &[ToolBinding],
) -> Result<ToolCallCounts> {
    let counts = ToolCallCounts::new(bindings.iter().map(|b| b.alias().to_owned()));
    let declared: Vec<String> = bound_tools
        .bindings()
        .iter()
        .map(|binding| binding.alias().to_owned())
        .collect();
    install_lua_tool_calls(lua, &counts, &declared)?;
    Ok(counts)
}

/// Installs the H2 tool declaration and local-tool APIs into one section VM.
///
/// The suspending `tools.call` is not installed here: yield cannot cross
/// the Rust callback boundary, so the coroutine shim prelude installs it on
/// this table as a Lua function.
///
/// # Errors
/// Returns [`Error::Lua`] if a Lua table or callback cannot be created or
/// installed.
pub(crate) fn install_h2_tools(
    lua: &Lua,
    globals: &Table,
    bindings: &ToolSet,
    runtime: &Arc<Mutex<ToolRuntime>>,
    local_tools: &LocalTools,
) -> Result<()> {
    let tools = lua.create_table().map_err(Error::lua)?;
    for name in ["bind", "always"] {
        let operation = name;
        let forbidden = lua
            .create_function(move |_, _: MultiValue| -> mlua::Result<()> {
                Err(mlua::Error::external(format!(
                    "tools.{operation} is only available during live H1 execution"
                )))
            })
            .map_err(Error::lua)?;
        tools.set(name, forbidden).map_err(Error::lua)?;
    }

    let frozen = bindings.clone();
    let state = Arc::clone(runtime);
    let add = lua
        .create_function(move |_, args: Variadic<Value>| {
            let entries = collect_tools_add_entries(args)?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            for entry in &entries {
                validate_alias(&entry.alias).map_err(mlua::Error::external)?;
                if frozen.binding(&entry.alias).is_none() {
                    return Err(mlua::Error::external(format!(
                        "tools.add alias {:?} was not declared by tools.bind",
                        entry.alias
                    )));
                }
            }
            for entry in entries {
                if let Some(description) = entry.description_override {
                    state
                        .description_overrides
                        .insert(entry.alias.clone(), description);
                }
                if frozen
                    .always
                    .iter()
                    .any(|existing| existing == &entry.alias)
                {
                    continue;
                }
                if !state.added.iter().any(|existing| existing == &entry.alias) {
                    state.added.push(entry.alias);
                }
            }
            Ok(())
        })
        .map_err(Error::lua)?;
    tools.set("add", add).map_err(Error::lua)?;

    let declared = bindings.clone();
    let local = local_tools.clone();
    let add_local_fn = lua
        .create_function(
            move |lua, (alias, description, params, handler): (String, String, Table, Function)| {
                validate_alias(&alias).map_err(mlua::Error::external)?;
                if declared.binding(&alias).is_some() {
                    return Err(mlua::Error::external(format!(
                        "tools.add_local alias {alias:?} duplicates a declared tool alias"
                    )));
                }
                if local.contains(&alias).map_err(mlua::Error::external)? {
                    return Err(mlua::Error::external(format!(
                        "tools.add_local alias {alias:?} is already registered"
                    )));
                }
                let parameters = add_local_params_schema(&params)?;
                let schema = ToolSchema::new(alias.clone(), description, parameters)
                    .map_err(mlua::Error::external)?;
                let key = lua.create_registry_value(handler)?;
                local
                    .register(alias, schema, key)
                    .map_err(mlua::Error::external)?;
                Ok(())
            },
        )
        .map_err(Error::lua)?;
    tools.set("add_local", add_local_fn).map_err(Error::lua)?;

    globals.raw_set("tools", tools).map_err(Error::lua)
}

#[cfg(test)]
mod tests;
