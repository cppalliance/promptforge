//! The `tools` namespace: scoping, invocation, and counts.
//!
//! One Lua table holds every tool operation, mirroring the `models.*`
//! namespacing of model operations. The shared [`ToolSet`] holds every
//! catalog tool the run can offer, each bound under its wire name: its id
//! with `/` and `.` replaced by `_`, such as `web_fetch`. The model sees
//! only wire names; Lua names a tool by its canonical id (`web/fetch`) or
//! by its tool object, and never by a global. `required` and `extras` list
//! the tool objects of the declared Plugins and of every other Plugin,
//! `get` reads one by id, `offer` scopes tools into the section,
//! `always_offer` offers tools prompt-wide (conventionally from H1, not
//! privileged to it), `offer_local` registers a prompt-author Lua function
//! as a tool, `call` dispatches a catalog or local tool (installed by the
//! coroutine shim prelude, since dispatch suspends), `allow_tasks` records
//! the section's allowlist for the model's task built-ins, and `calls` is
//! the read-only dispatch counter surface, keyed by id for a catalog tool
//! and by alias for a local one. Offering an id the run cannot offer is a
//! hard error. The installation logic sits here, out of the VM driver;
//! the VM only calls the installers in setup order.

use std::sync::{Arc, Mutex};

use mlua::{Function, Lua, MultiValue, Table, Value, Variadic};
use promptforge_model_client::detail::tool_schema_new;
use promptforge_types::tools::ToolId;

use crate::alias::validate_alias;
use crate::error::{Error, Result};
use crate::handles::{ToolBinding, ToolSet};
use crate::scope::{TaskAllowlist, ToolCallCounts, ToolRuntime};
use crate::vm::LocalTools;

mod decode;
mod objects;
mod userdata;

pub(crate) use decode::tool_alias;
#[cfg(test)]
pub(crate) use userdata::LuaToolHandle;

use decode::{ToolsAddEntry, add_local_params_schema, collect_tools_add_entries};
pub(crate) use objects::tool_object;
use objects::{install_tool_lists, install_tool_objects};

/// The registry key of the VM's local-tool handler table:
/// `tools.offer_local` writes each handler under its alias, and the
/// scheduler's `Local` answer reads it back to hand the function to the
/// shim.
const LOCAL_HANDLERS_REGISTRY: &str = "promptforge.tools.local_handlers";

/// Reads the handler `tools.offer_local` registered under `alias` from the
/// VM's handler table.
///
/// # Errors
/// Returns an internal-invariant error when the table is absent or holds no
/// function under `alias`: the scheduler answers `Local` only for an alias
/// the VM reports as registered.
pub(crate) fn local_handler(lua: &Lua, alias: &str) -> mlua::Result<Function> {
    let handlers: Option<Table> = lua.named_registry_value(LOCAL_HANDLERS_REGISTRY)?;
    match handlers
        .map(|handlers| handlers.raw_get::<Value>(alias))
        .transpose()?
    {
        Some(Value::Function(handler)) => Ok(handler),
        _ => Err(mlua::Error::external(Error::Internal(
            "a local tool answer names a registered handler",
        ))),
    }
}

/// Locks the run's shared tool set, mapping a poisoned lock to the Lua
/// boundary error every Engine function uses.
fn lock_tools(set: &Mutex<ToolSet>) -> mlua::Result<std::sync::MutexGuard<'_, ToolSet>> {
    set.lock()
        .map_err(|_| mlua::Error::external("tool set mutex was poisoned"))
}

/// Installs the read-only `tools.calls` counter table over `counts`;
/// `offered`, the ids of every tool the run can offer, feeds the
/// unknown-key diagnostic.
///
/// # Errors
/// Returns [`Error::Lua`] if the Lua table or callbacks cannot be created or
/// installed.
fn install_lua_tool_calls(lua: &Lua, counts: &ToolCallCounts, offered: &[String]) -> Result<()> {
    let globals = lua.globals();
    let tools: Table = globals.raw_get("tools").map_err(Error::lua)?;

    let calls_inner = lua.create_table().map_err(Error::lua)?;
    let meta = lua.create_table().map_err(Error::lua)?;

    let counts_for_index = counts.clone();
    let offered: Vec<String> = offered.to_vec();
    let index = lua
        .create_function(move |_, (_table, key): (Table, String)| {
            let value = counts_for_index.get(&key).map_err(mlua::Error::external)?;
            if let Some(count) = value {
                Ok(count)
            } else {
                let seeded = counts_for_index.aliases().map_err(mlua::Error::external)?;
                let offered_unseeded = offered.iter().any(|id| id == &key);
                Err(mlua::Error::external(format!(
                    "tools.calls: {key:?} has no seeded count; \
                     seeded names: {seeded:?}{}",
                    if offered_unseeded {
                        " (a catalog tool that was neither offered in this section \
                         nor called with tools.call)"
                    } else if seeded.is_empty() {
                        ""
                    } else {
                        " - check for typos or offer it with tools.offer"
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
/// [`ToolCallCounts`] seeded with the ids of `bindings`. Each seeded key
/// reads its live count; indexing an unseeded key is a hard error that
/// names the bad key and lists the seeded set. When the key is the id of a
/// tool the run offers but was never seeded - neither offered to the
/// section nor dispatched by a script `tools.call` - the diagnostic says
/// so.
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
    let counts = ToolCallCounts::new(bindings.iter().map(|b| b.id().to_string()));
    let offered: Vec<String> = bound_tools
        .offered()
        .iter()
        .map(|binding| binding.id().to_string())
        .collect();
    install_lua_tool_calls(lua, &counts, &offered)?;
    Ok(counts)
}

/// Resolves each of `call`'s entries to its offered binding's wire name,
/// checking every entry before the caller records any: an entry must parse
/// as a tool id, so a wire name, a malformed id, or a local alias is
/// refused as an id the run does not offer.
fn resolve_entries(
    call: &str,
    set: &ToolSet,
    entries: Vec<ToolsAddEntry>,
) -> mlua::Result<Vec<(String, Option<String>)>> {
    entries
        .into_iter()
        .map(|entry| {
            let binding = ToolId::parse(&entry.alias)
                .ok()
                .and_then(|_| set.offered_binding(&entry.alias))
                .ok_or_else(|| {
                    mlua::Error::external(format!(
                        "{call}: {:?} is not a catalog tool in this run",
                        entry.alias
                    ))
                })?;
            Ok((binding.alias().to_owned(), entry.description_override))
        })
        .collect()
}

/// Installs the tool scoping and local-tool APIs into one section VM (H1
/// included: there is one install path for every section).
///
/// The coroutine shim prelude installs the suspending `tools.call` on this
/// table as a Lua function, because yield cannot cross the Rust callback
/// boundary.
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
    install_tool_objects(lua, &*lock_tools(set).map_err(Error::lua)?)?;
    install_offer(lua, &tools, set, runtime)?;
    install_always_offer(lua, &tools, set)?;
    install_tool_lists(lua, &tools, set)?;
    install_offer_local(lua, &tools, local_tools)?;
    install_allow_tasks(lua, &tools, runtime)?;

    globals.raw_set("tools", tools).map_err(Error::lua)
}

/// Installs `tools.offer(tool, override?)` and its array form: each tool
/// enters the section's scope once, under its wire name, in first-offer
/// order, with any override recorded for the section. A tool already
/// offered prompt-wide keeps its prompt-wide place.
fn install_offer(
    lua: &Lua,
    tools: &Table,
    set: &Arc<Mutex<ToolSet>>,
    runtime: &Arc<Mutex<ToolRuntime>>,
) -> Result<()> {
    let frozen = Arc::clone(set);
    let state = Arc::clone(runtime);
    let offer = lua
        .create_function(move |_, args: Variadic<Value>| {
            let entries = collect_tools_add_entries("tools.offer", args)?;
            let resolved = resolve_entries("tools.offer", &*lock_tools(&frozen)?, entries)?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            let set = lock_tools(&frozen)?;
            for (name, description) in resolved {
                if let Some(description) = description {
                    state
                        .description_overrides
                        .insert(name.clone(), description);
                }
                if set.always.iter().any(|existing| existing == &name) {
                    continue;
                }
                if !state.added.iter().any(|existing| existing == &name) {
                    state.added.push(name);
                }
            }
            Ok(())
        })
        .map_err(Error::lua)?;
    tools.set("offer", offer).map_err(Error::lua)
}

/// Installs `tools.always_offer(tool, override?)` and its array form: each
/// tool is offered in every section from here on, its wire name recorded
/// once in the run's prompt-wide list, with any override set on its
/// binding for the whole run.
fn install_always_offer(lua: &Lua, tools: &Table, set: &Arc<Mutex<ToolSet>>) -> Result<()> {
    let frozen = Arc::clone(set);
    let always_offer = lua
        .create_function(move |_, args: Variadic<Value>| {
            let entries = collect_tools_add_entries("tools.always_offer", args)?;
            let mut set = lock_tools(&frozen)?;
            let resolved = resolve_entries("tools.always_offer", &set, entries)?;
            for (name, description) in resolved {
                if let Some(description) = description
                    && let Some(binding) =
                        set.offered.iter_mut().find(|binding| binding.alias == name)
                {
                    binding.model_description = Some(description);
                }
                // Idempotent under the shared-library replay: every section
                // re-runs the library, so offering the same tool again is a
                // no-op.
                if !set.always.iter().any(|existing| existing == &name) {
                    set.always.push(name);
                }
            }
            Ok(())
        })
        .map_err(Error::lua)?;
    tools.set("always_offer", always_offer).map_err(Error::lua)
}

/// Installs `tools.offer_local(alias, description, params, handler)` over
/// a fresh handler table: the alias and schema register on `local_tools`
/// for membership and advertising, and the handler is written under the
/// alias for the scheduler's `Local` answer to read back. An alias may
/// equal an offered tool's wire name; the local tool wins in scope.
fn install_offer_local(lua: &Lua, tools: &Table, local_tools: &LocalTools) -> Result<()> {
    lua.set_named_registry_value(
        LOCAL_HANDLERS_REGISTRY,
        lua.create_table().map_err(Error::lua)?,
    )
    .map_err(Error::lua)?;
    let local = local_tools.clone();
    let offer_local = lua
        .create_function(
            move |lua, (alias, description, params, handler): (String, String, Table, Function)| {
                validate_alias(&alias).map_err(mlua::Error::external)?;
                if local.contains(&alias).map_err(mlua::Error::external)? {
                    return Err(mlua::Error::external(format!(
                        "tools.offer_local alias {alias:?} is already registered"
                    )));
                }
                let parameters = add_local_params_schema(&params)?;
                let schema = tool_schema_new(alias.clone(), description, parameters)
                    .map_err(mlua::Error::external)?;
                let handlers: Table = lua.named_registry_value(LOCAL_HANDLERS_REGISTRY)?;
                handlers.raw_set(alias.as_str(), handler)?;
                local
                    .register(alias, schema)
                    .map_err(mlua::Error::external)?;
                Ok(())
            },
        )
        .map_err(Error::lua)?;
    tools.set("offer_local", offer_local).map_err(Error::lua)
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
