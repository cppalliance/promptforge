use super::{
    Arc, Error, Function, Json, LocalTools, Lua, LuaToolHandle, MultiValue, Mutex, Result,
    ToolCallCounts, ToolRuntime, ToolSet, Value, Variadic, json, validate_alias,
};
use promptforge_model_client::client::ToolSchema;

/// Installs the read-only `tools.calls` counter table for declared aliases.
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
    let tools: mlua::Table = globals.raw_get("tools").map_err(Error::lua)?;

    let calls_inner = lua.create_table().map_err(Error::lua)?;
    let meta = lua.create_table().map_err(Error::lua)?;

    let counts_for_index = counts.clone();
    let declared: Vec<String> = declared.to_vec();
    let index = lua
        .create_function(move |_, (_table, key): (mlua::Table, String)| {
            let value = counts_for_index.get(&key).map_err(mlua::Error::external)?;
            if let Some(count) = value {
                Ok(count)
            } else {
                let in_scope = counts_for_index.aliases().map_err(mlua::Error::external)?;
                let declared_unscoped = declared.iter().any(|alias| alias == &key);
                Err(mlua::Error::external(format!(
                    "tools.calls: {key:?} is not in this section's tool scope; \
                     in-scope aliases: {in_scope:?}{}",
                    if declared_unscoped {
                        " (alias was declared by tools.bind but not added to this section's scope)"
                    } else if in_scope.is_empty() {
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

/// One flattened `tools.add` entry: alias plus optional model-description override.
pub(crate) struct ToolsAddEntry {
    alias: String,
    description_override: Option<String>,
}

/// Reads one `tools.add` element as an alias: a string or a Tool handle.
fn add_alias(value: Value) -> mlua::Result<String> {
    match value {
        Value::String(s) => Ok(s.to_string_lossy()),
        Value::UserData(ud) => Ok(ud.borrow::<LuaToolHandle>()?.name().to_owned()),
        other => Err(mlua::Error::external(format!(
            "tools.add expects strings, Tool objects, or arrays of either, got {}",
            other.type_name()
        ))),
    }
}

/// Flattens the `tools.add` arguments into alias/override entries.
///
/// `tools.add(alias, override?)` takes one alias (string or Tool handle) with
/// an optional model-description override. The array form
/// `tools.add({"a", "b"})` covers bulk and takes no per-element overrides.
pub(crate) fn collect_tools_add_entries(args: Variadic<Value>) -> mlua::Result<Vec<ToolsAddEntry>> {
    let mut args = args.into_iter();
    let Some(target) = args.next() else {
        return Ok(Vec::new());
    };
    let description_override = match args.next() {
        None => None,
        Some(Value::String(s)) => Some(s.to_string_lossy()),
        Some(other) => {
            return Err(mlua::Error::external(format!(
                "tools.add override must be a string, got {}",
                other.type_name()
            )));
        }
    };
    if let Some(extra) = args.next() {
        return Err(mlua::Error::external(format!(
            "tools.add takes one alias plus an optional override, got extra {}",
            extra.type_name()
        )));
    }
    match target {
        Value::Table(table) => {
            if description_override.is_some() {
                return Err(mlua::Error::external(
                    "tools.add array form takes no override",
                ));
            }
            table
                .sequence_values::<Value>()
                .map(|item| {
                    Ok(ToolsAddEntry {
                        alias: add_alias(item?)?,
                        description_override: None,
                    })
                })
                .collect()
        }
        single => Ok(vec![ToolsAddEntry {
            alias: add_alias(single)?,
            description_override,
        }]),
    }
}

/// Builds the JSON Schema `parameters` object from a `tools.add_local` params
/// table. Each value is a bare type string or a `{type, description}` array;
/// every declared parameter is required.
fn add_local_params_schema(params: &mlua::Table) -> mlua::Result<Json> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for pair in params.pairs::<String, Value>() {
        let (name, spec) = pair?;
        let (ty, description) = match spec {
            Value::String(s) => (s.to_string_lossy(), None),
            Value::Table(t) => (t.get::<String>(1)?, t.get::<Option<String>>(2)?),
            _ => {
                return Err(mlua::Error::external(format!(
                    "tools.add_local param {name:?} must be a type string or a {{type, description}} array"
                )));
            }
        };
        if !matches!(ty.as_str(), "string" | "integer" | "number" | "boolean") {
            return Err(mlua::Error::external(format!(
                "tools.add_local param {name:?} has unsupported type {ty:?}: \
                 expected \"string\", \"integer\", \"number\", or \"boolean\""
            )));
        }
        let mut property = json!({ "type": ty });
        if let Some(description) = description {
            property["description"] = Json::String(description);
        }
        properties.insert(name.clone(), property);
        required.push(Json::String(name));
    }
    Ok(json!({
        "type": "object",
        "properties": properties,
        "required": required,
    }))
}

/// Installs the H2 tool declaration and local-tool APIs into one section VM.
///
/// # Errors
/// Returns [`Error::Lua`] if a Lua table or callback cannot be created or
/// installed.
pub(crate) fn install_h2_tools(
    lua: &Lua,
    globals: &mlua::Table,
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
            move |lua,
                  (alias, description, params, handler): (
                String,
                String,
                mlua::Table,
                Function,
            )| {
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
