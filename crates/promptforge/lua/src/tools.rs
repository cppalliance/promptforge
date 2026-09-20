//! The `tools` namespace: scoping, invocation, and counts.
//!
//! One Lua table carries every tool operation, mirroring the `models.*`
//! namespacing of model operations. Binding is the frontmatter's: the run's
//! filled slots arrive in the shared [`ToolSet`], and the table scopes among
//! them by alias - `add` scopes aliases into the section, `always` parks a
//! prompt-wide alias (conventionally from H1, not privileged to it),
//! `add_local` registers a prompt-author Lua function as a tool, `call`
//! dispatches a bound tool by alias or Tool object (installed by the
//! coroutine shim prelude, since dispatch suspends), `allow_tasks` records
//! the section's allowlist for the model's task built-ins, and `calls` is
//! the read-only per-alias dispatch counter surface. Only filled slots are
//! visible: scoping or advertising an unfilled alias is a hard error. The
//! installation logic lives here, out of the VM driver; the VM only calls
//! the installers in setup order.

use std::sync::{Arc, Mutex};

use mlua::{Function, Lua, MultiValue, Table, Value, Variadic};
use promptforge_model_client::client::ToolSchema;

use crate::alias::validate_alias;
use crate::error::{Error, Result};
use crate::handles::{ToolBinding, ToolSet};
use crate::scope::{TaskAllowlist, ToolCallCounts, ToolRuntime};
use crate::vm::LocalTools;

mod decode;
mod userdata;

pub(crate) use userdata::LuaToolHandle;

pub(crate) use decode::tool_alias;

use decode::{add_local_params_schema, collect_tools_add_entries};

/// Locks the run's shared tool set, mapping a poisoned lock to the Lua
/// boundary error every host callback uses.
fn lock_tools(set: &Mutex<ToolSet>) -> mlua::Result<std::sync::MutexGuard<'_, ToolSet>> {
    set.lock()
        .map_err(|_| mlua::Error::external("tool set mutex was poisoned"))
}

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
                        " (alias is a bound tool slot but was neither added to \
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
/// set. When the key names a bound tool slot but was never seeded - neither
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

/// Installs the tool scoping and local-tool APIs into one section VM (H1
/// included: there is one install path for every section).
///
/// The suspending `tools.call` is not installed here: yield cannot cross
/// the Rust callback boundary, so the coroutine shim prelude installs it on
/// this table as a Lua function.
///
/// # Errors
/// Returns [`Error::Lua`] if a Lua table or callback cannot be created or
/// installed.
pub(crate) fn install_tools(
    lua: &Lua,
    globals: &Table,
    set: &Arc<Mutex<ToolSet>>,
    runtime: &Arc<Mutex<ToolRuntime>>,
    local_tools: &LocalTools,
) -> Result<()> {
    let tools = lua.create_table().map_err(Error::lua)?;

    let frozen = Arc::clone(set);
    let state = Arc::clone(runtime);
    let add = lua
        .create_function(move |_, args: Variadic<Value>| {
            let entries = collect_tools_add_entries(args)?;
            {
                let set = lock_tools(&frozen)?;
                for entry in &entries {
                    validate_alias(&entry.alias).map_err(mlua::Error::external)?;
                    if set.binding(&entry.alias).is_none() {
                        return Err(mlua::Error::external(format!(
                            "tools.add alias {:?} is not a bound tool slot",
                            entry.alias
                        )));
                    }
                }
            }
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            let set = lock_tools(&frozen)?;
            for entry in entries {
                if let Some(description) = entry.description_override {
                    state
                        .description_overrides
                        .insert(entry.alias.clone(), description);
                }
                if set.always.iter().any(|existing| existing == &entry.alias) {
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

    let frozen = Arc::clone(set);
    let always = lua
        .create_function(
            move |_, (alias, model_description): (String, Option<String>)| -> mlua::Result<()> {
                validate_alias(&alias).map_err(mlua::Error::external)?;
                let mut set = lock_tools(&frozen)?;
                let Some(binding) = set
                    .bindings
                    .iter_mut()
                    .find(|binding| binding.alias == alias)
                else {
                    return Err(mlua::Error::external(format!(
                        "tools.always alias {alias:?} is not a bound tool slot"
                    )));
                };
                if let Some(model_description) = model_description {
                    binding.model_description = Some(model_description);
                }
                // Idempotent under the shared-library replay: every section
                // re-runs the library, so naming the same alias again is a
                // no-op.
                if !set.always.iter().any(|existing| existing == &alias) {
                    set.always.push(alias);
                }
                Ok(())
            },
        )
        .map_err(Error::lua)?;
    tools.set("always", always).map_err(Error::lua)?;

    let declared = Arc::clone(set);
    let local = local_tools.clone();
    let add_local_fn = lua
        .create_function(
            move |lua, (alias, description, params, handler): (String, String, Table, Function)| {
                validate_alias(&alias).map_err(mlua::Error::external)?;
                if lock_tools(&declared)?.binding(&alias).is_some() {
                    return Err(mlua::Error::external(format!(
                        "tools.add_local alias {alias:?} duplicates a bound tool slot"
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
    install_allow_tasks(lua, &tools, runtime)?;

    globals.raw_set("tools", tools).map_err(Error::lua)
}

/// Installs `tools.allow_tasks(targets?)`, the author's opt-in for the
/// model's task built-ins: the decoded allowlist is recorded on the
/// section's tool runtime. The latest call is the section's allowlist - a
/// second call replaces rather than unions, so a library's broad grant can
/// be narrowed by the section that follows it.
fn install_allow_tasks(lua: &Lua, tools: &Table, runtime: &Arc<Mutex<ToolRuntime>>) -> Result<()> {
    let state = Arc::clone(runtime);
    let allow_tasks = lua
        .create_function(move |_, targets: Option<Value>| {
            let allowlist = task_allowlist_from(targets)?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            state.allowed_tasks = Some(allowlist);
            Ok(())
        })
        .map_err(Error::lua)?;
    tools.set("allow_tasks", allow_tasks).map_err(Error::lua)
}

/// Decodes `tools.allow_tasks`'s argument: absent for any target, else a
/// non-empty sequence of non-empty heading strings.
fn task_allowlist_from(targets: Option<Value>) -> mlua::Result<TaskAllowlist> {
    let table = match targets {
        None | Some(Value::Nil) => return Ok(TaskAllowlist::Any),
        Some(Value::Table(table)) => table,
        Some(other) => {
            return Err(mlua::Error::external(format!(
                "tools.allow_tasks targets must be a list of section headings, got {}",
                other.type_name()
            )));
        }
    };
    let mut headings = Vec::new();
    for entry in table.sequence_values::<Value>() {
        match entry? {
            Value::String(heading) => {
                let heading = heading.to_str()?.trim().to_owned();
                if heading.is_empty() {
                    return Err(mlua::Error::external(
                        "tools.allow_tasks targets must be non-empty section headings",
                    ));
                }
                headings.push(heading);
            }
            other => {
                return Err(mlua::Error::external(format!(
                    "tools.allow_tasks targets must be section heading strings, got {}",
                    other.type_name()
                )));
            }
        }
    }
    if headings.is_empty() {
        return Err(mlua::Error::external(
            "tools.allow_tasks targets must name at least one section; \
             call it with no argument to allow any section",
        ));
    }
    Ok(TaskAllowlist::Only(headings))
}

#[cfg(test)]
mod tests;
