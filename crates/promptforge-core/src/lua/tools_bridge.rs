use super::*;

pub(crate) fn install_lua_tool_calls(
    lua: &Lua,
    counts: &ToolCallCounts,
    declared: &[String],
) -> Result<()> {
    let globals = lua.globals();
    let tools: mlua::Table = globals
        .raw_get("tools")
        .map_err(|error| Error::Lua(error.to_string()))?;

    let calls_inner = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;
    let meta = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let counts_for_index = counts.clone();
    let declared: Vec<String> = declared.to_vec();
    let index = lua
        .create_function(move |_, (_table, key): (mlua::Table, String)| {
            let value = counts_for_index
                .get(&key)
                .map_err(|e| mlua::Error::external(e.to_string()))?;
            if let Some(count) = value {
                Ok(count)
            } else {
                let in_scope = counts_for_index
                    .aliases()
                    .map_err(|e| mlua::Error::external(e.to_string()))?;
                let declared_unscoped = declared.iter().any(|alias| alias == &key);
                Err(mlua::Error::external(format!(
                    "tools.calls: {key:?} is not in this section's tool scope; \
                     in-scope aliases: {in_scope:?}{}",
                    if declared_unscoped {
                        " (alias was declared by tools.need but not added to this section's scope)"
                    } else if in_scope.is_empty() {
                        ""
                    } else {
                        " - check for typos or add it via tools.add"
                    }
                )))
            }
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    meta.set("__index", index)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let newindex_err = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external("tools.calls is read-only"))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    meta.set("__newindex", newindex_err)
        .map_err(|error| Error::Lua(error.to_string()))?;

    calls_inner.set_metatable(Some(meta));

    tools
        .set("calls", calls_inner)
        .map_err(|error| Error::Lua(error.to_string()))?;
    Ok(())
}

/// One flattened `tools.add` entry: alias plus optional model-description override.
pub(crate) struct ToolsAddEntry {
    alias: String,
    description_override: Option<String>,
}

/// Collects add entries from one `tools.add` argument.
///
/// Accepts a UTF-8 string, a [`LuaToolHandle`], or a sequence table of either.
/// A Tool handle contributes a description override only when the author
/// assigned `.description` on that object.
pub(crate) fn push_tools_add_entry(
    entries: &mut Vec<ToolsAddEntry>,
    value: Value,
) -> mlua::Result<()> {
    match value {
        Value::String(s) => {
            entries.push(ToolsAddEntry {
                alias: s.to_string_lossy(),
                description_override: None,
            });
            Ok(())
        }
        Value::UserData(ud) => {
            let handle = ud.borrow::<LuaToolHandle>()?;
            entries.push(ToolsAddEntry {
                alias: handle.name().to_owned(),
                description_override: handle.model_description_override().map(str::to_owned),
            });
            Ok(())
        }
        Value::Table(table) => {
            for item in table.sequence_values::<Value>() {
                match item? {
                    Value::String(s) => entries.push(ToolsAddEntry {
                        alias: s.to_string_lossy(),
                        description_override: None,
                    }),
                    Value::UserData(ud) => {
                        let handle = ud.borrow::<LuaToolHandle>()?;
                        entries.push(ToolsAddEntry {
                            alias: handle.name().to_owned(),
                            description_override: handle
                                .model_description_override()
                                .map(str::to_owned),
                        });
                    }
                    _ => {
                        return Err(mlua::Error::external(
                            "tools.add array elements must be strings or Tool objects",
                        ));
                    }
                }
            }
            Ok(())
        }
        _ => Err(mlua::Error::external(
            "tools.add expects strings, Tool objects, or arrays of either",
        )),
    }
}

/// Flattens a `tools.add` variadic into alias/override entries for scope.
pub(crate) fn collect_tools_add_entries(args: Variadic<Value>) -> mlua::Result<Vec<ToolsAddEntry>> {
    let mut entries = Vec::new();
    for value in args {
        push_tools_add_entry(&mut entries, value)?;
    }
    Ok(entries)
}

pub(crate) fn install_h2_tools(
    lua: &Lua,
    globals: &mlua::Table,
    bindings: &ToolBindings,
    runtime: &Arc<Mutex<ToolRuntime>>,
) -> Result<()> {
    {
        let state = runtime
            .lock()
            .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
        if state.phase != ToolPhase::H2 {
            return Err(Error::Lua(
                "tool scope is not open for H2 recording".to_owned(),
            ));
        }
    }

    let tools = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;
    for name in ["need", "always"] {
        let operation = name;
        let forbidden = lua
            .create_function(move |_, _: MultiValue| -> mlua::Result<()> {
                Err(mlua::Error::external(format!(
                    "tools.{operation} is only available during live H1 execution"
                )))
            })
            .map_err(|error| Error::Lua(error.to_string()))?;
        tools
            .set(name, forbidden)
            .map_err(|error| Error::Lua(error.to_string()))?;
    }

    let frozen = bindings.clone();
    let state = Arc::clone(runtime);
    let add = lua
        .create_function(move |_, args: Variadic<Value>| {
            let entries = collect_tools_add_entries(args)?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("tool declaration runtime was poisoned"))?;
            if state.phase != ToolPhase::H2 {
                return Err(mlua::Error::external(
                    "tools.add is only available before the H2 tool scope closes",
                ));
            }
            for entry in &entries {
                validate_alias(&entry.alias)
                    .map_err(|error| mlua::Error::external(error.to_string()))?;
                if frozen.binding(&entry.alias).is_none() {
                    return Err(mlua::Error::external(format!(
                        "tools.add alias {:?} was not declared by tools.need",
                        entry.alias
                    )));
                }
            }
            let mut changed = false;
            for entry in entries {
                if let Some(description) = entry.description_override {
                    let override_changed = match state.description_overrides.get(&entry.alias) {
                        Some(existing) => existing != &description,
                        None => true,
                    };
                    if override_changed {
                        state
                            .description_overrides
                            .insert(entry.alias.clone(), description);
                        changed = true;
                    }
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
                    changed = true;
                }
            }
            if changed {
                state.generation = state.generation.saturating_add(1);
            }
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    tools
        .set("add", add)
        .map_err(|error| Error::Lua(error.to_string()))?;
    globals
        .raw_set("tools", tools)
        .map_err(|error| Error::Lua(error.to_string()))
}

/// Installs the phase-local author diagnostic callback.
///
/// The callback borrows its observer through [`Scope`], so neither the callback
/// nor any Lua reference copied from it can retain that observer after the
/// current H1 or H2 phase returns.
pub(crate) fn install_tasks_table(lua: &Lua, tasks: &[LuaSectionHandle]) -> Result<()> {
    let table = lua
        .create_table_with_capacity(0, tasks.len())
        .map_err(|error| Error::Lua(error.to_string()))?;
    for handle in tasks {
        let userdata = lua
            .create_userdata(handle.clone())
            .map_err(|error| Error::Lua(error.to_string()))?;
        table
            .raw_set(handle.heading(), userdata)
            .map_err(|error| Error::Lua(error.to_string()))?;
    }
    lua.globals()
        .raw_set("tasks", table)
        .map_err(|error| Error::Lua(error.to_string()))
}
