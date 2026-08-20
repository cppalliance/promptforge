use super::{Error, Json, Lua, LuaSerdeExt, ModelBinding, Result, Value};

/// The registry key holding the `var` proxy's hidden data table.
///
/// The registry is unreachable from sandboxed author code (no `debug`
/// library), so only host code holding a `&Lua` can read or replace the data
/// table behind the guarded global.
const VAR_DATA_REGISTRY: &str = "promptforge.var_data";

/// The registry key holding the `var` proxy table itself, so read-back can
/// detect an author reassigning the global (`var = 5`), which would
/// otherwise silently strand the hidden data table.
const VAR_PROXY_REGISTRY: &str = "promptforge.var_proxy";

/// Registry map from every guarded proxy table to its hidden data table.
const VAR_GUARDED_DATA_REGISTRY: &str = "promptforge.var_guarded_data";

fn field_path(path: &str, key: &Value) -> String {
    match key {
        Value::String(name) => format!("{path}.{}", name.to_string_lossy()),
        other => format!("{path}[{other:?}]"),
    }
}

fn guarded_json_value(
    lua: &Lua,
    value: &Json,
    path: &str,
    guarded_data: &mlua::Table,
) -> mlua::Result<Value> {
    match value {
        Json::Array(values) => {
            let data = lua.create_table_with_capacity(values.len(), 0)?;
            for (index, value) in values.iter().enumerate() {
                data.raw_set(
                    index + 1,
                    guarded_json_value(
                        lua,
                        value,
                        &format!("{path}[{}]", index + 1),
                        guarded_data,
                    )?,
                )?;
            }
            guarded_table(lua, data, path, guarded_data).map(Value::Table)
        }
        Json::Object(values) => {
            let data = lua.create_table_with_capacity(0, values.len())?;
            for (key, value) in values {
                data.raw_set(
                    key.as_str(),
                    guarded_json_value(lua, value, &format!("{path}.{key}"), guarded_data)?,
                )?;
            }
            guarded_table(lua, data, path, guarded_data).map(Value::Table)
        }
        _ => lua.to_value(value),
    }
}

fn guarded_table(
    lua: &Lua,
    data: mlua::Table,
    path: &str,
    guarded_data: &mlua::Table,
) -> mlua::Result<mlua::Table> {
    let proxy = lua.create_table()?;
    guarded_data.raw_set(proxy.clone(), data.clone())?;
    let metatable = lua.create_table()?;
    metatable.set("__index", data.clone())?;

    let write_data = data;
    let nested_data = guarded_data.clone();
    let table_path = path.to_owned();
    let newindex = lua.create_function(
        move |lua, (_proxy, key, value): (Value, Value, Value)| -> mlua::Result<()> {
            let target = field_path(&table_path, &key);
            if let Value::Function(_) | Value::UserData(_) | Value::Thread(_) = value {
                return Err(mlua::Error::runtime(format!(
                    "{target} must be JSON data, got {}",
                    value.type_name()
                )));
            }
            let json = lua.from_value::<Json>(value)?;
            let rebuilt = guarded_json_value(lua, &json, &target, &nested_data)?;
            write_data.raw_set(key, rebuilt)
        },
    )?;
    metatable.set("__newindex", newindex)?;
    metatable.set("__metatable", "var is guarded")?;
    proxy.set_metatable(Some(metatable));
    Ok(proxy)
}

fn materialize_guarded_value(
    lua: &Lua,
    value: Value,
    guarded_data: &mlua::Table,
) -> mlua::Result<Value> {
    let Value::Table(table) = value else {
        return Ok(value);
    };
    let source = match guarded_data.raw_get::<Value>(table.clone())? {
        Value::Table(data) => data,
        _ => table,
    };
    let plain = lua.create_table()?;
    for pair in source.pairs::<Value, Value>() {
        let (key, value) = pair?;
        plain.raw_set(key, materialize_guarded_value(lua, value, guarded_data)?)?;
    }
    Ok(Value::Table(plain))
}

/// Returns a copy of `sys` with one object field replaced.
fn enrich_sys_field(sys: &Json, key: &str, value: Json) -> Json {
    match sys {
        Json::Object(map) => {
            let mut out = map.clone();
            out.insert(key.to_owned(), value);
            Json::Object(out)
        }
        other => other.clone(),
    }
}

/// Returns a copy of `sys` with the bound catalog model id under `"model"`.
pub(crate) fn enrich_sys_model(sys: &Json, binding: &ModelBinding) -> Json {
    enrich_sys_field(sys, "model", Json::String(binding.id().name().to_owned()))
}

/// Returns a copy of `sys` with `reply_finish_reason` set from the last inference.
pub(crate) fn enrich_sys_reply_finish_reason(sys: &Json, reason: Option<&str>) -> Json {
    let value = match reason {
        Some(value) => Json::String(value.to_owned()),
        None => Json::Null,
    };
    enrich_sys_field(sys, "reply_finish_reason", value)
}

/// Builds a sealed Lua `sys` table from runtime metadata.
///
/// The proxy is empty; reads go through `__index` against the JSON object and
/// raise when the field is absent. Present `null` values surface as Lua nil.
/// `__newindex` rejects every write. `__metatable` is set so author code cannot
/// replace the seal.
pub(crate) fn seal_sys(lua: &Lua, sys: &Json) -> Result<mlua::Table> {
    let data = match sys {
        Json::Object(map) => map.clone(),
        other => {
            return Err(Error::Lua(format!("sys must be a table, got {other}")));
        }
    };

    let proxy = lua.create_table().map_err(Error::lua)?;
    let metatable = lua.create_table().map_err(Error::lua)?;

    let index = lua
        .create_function(move |lua, (_table, key): (Value, Value)| {
            let Value::String(name) = key else {
                return Err(mlua::Error::runtime(
                    "sys fields must be accessed by string key".to_owned(),
                ));
            };
            let field = name.to_string_lossy();
            match data.get(field.as_str()) {
                None => Err(mlua::Error::runtime(format!("unknown sys field '{field}'"))),
                Some(Json::Null) => Ok(Value::Nil),
                Some(value) => lua.to_value(value),
            }
        })
        .map_err(Error::lua)?;
    metatable.set("__index", index).map_err(Error::lua)?;

    let newindex = lua
        .create_function(
            move |_lua, (_table, key, _value): (Value, Value, Value)| -> mlua::Result<()> {
                let field = match key {
                    Value::String(name) => name.to_string_lossy(),
                    other => format!("{other:?}"),
                };
                Err(mlua::Error::runtime(format!(
                    "sys is read-only; cannot set '{field}'"
                )))
            },
        )
        .map_err(Error::lua)?;
    metatable.set("__newindex", newindex).map_err(Error::lua)?;
    metatable
        .set("__metatable", "sys is sealed")
        .map_err(Error::lua)?;

    proxy.set_metatable(Some(metatable));
    Ok(proxy)
}

/// Builds the guarded `var` global: an empty proxy table over a hidden data
/// table.
///
/// The proxy is empty; reads go through `__index` against the data table, so
/// present values surface directly and absent keys read as Lua nil.
/// `__newindex` validates the assigned value for JSON-representability
/// through the serde bridge - a function, userdata, or thread is rejected at
/// the assigning line, and nested tables are deep-checked by the bridge -
/// then rebuilds it as fresh guarded data before writing through. Every nested
/// table is itself an empty proxy over hidden data, so later incremental writes
/// cross the same validation boundary instead of mutating a stored table
/// directly. `__metatable` is set so author code cannot replace the guard.
/// The root data table is stashed in the Lua
/// registry (unreachable from sandboxed author code) and is the read-back
/// source for [`var_to_json`], which materializes nested proxies before serde
/// conversion; proxies never hold entries themselves. The root proxy is
/// stashed alongside it so [`var_to_json`] can reject an author reassigning
/// the `var` global instead of silently reading stale data.
pub(crate) fn guarded_var(lua: &Lua, initial: Option<&Json>) -> Result<mlua::Table> {
    let initial = match initial {
        Some(Json::Object(values)) => Json::Object(values.clone()),
        Some(other) => {
            return Err(Error::Lua(format!(
                "var must seed from a JSON object, got {}",
                lua.to_value(other).map_err(Error::lua)?.type_name()
            )));
        }
        None => Json::Object(serde_json::Map::new()),
    };
    let guarded_data = lua.create_table().map_err(Error::lua)?;
    let Value::Table(proxy) =
        guarded_json_value(lua, &initial, "var", &guarded_data).map_err(Error::lua)?
    else {
        return Err(Error::Internal("guarded var root was not a table"));
    };
    let Value::Table(data) = guarded_data
        .raw_get::<Value>(proxy.clone())
        .map_err(Error::lua)?
    else {
        return Err(Error::Internal("guarded var data table was missing"));
    };
    // The named registry entries keep the data table alive and reachable for
    // host read-back, and the proxy reachable for the reassignment check in
    // `var_to_json`.
    lua.set_named_registry_value(VAR_DATA_REGISTRY, data)
        .map_err(Error::lua)?;
    lua.set_named_registry_value(VAR_PROXY_REGISTRY, proxy.clone())
        .map_err(Error::lua)?;
    lua.set_named_registry_value(VAR_GUARDED_DATA_REGISTRY, guarded_data)
        .map_err(Error::lua)?;
    Ok(proxy)
}

/// Reads the hidden `var` data table back as JSON.
///
/// # Errors
/// Returns [`Error::Lua`] if the data table is absent (host values were
/// never injected), if the author reassigned the `var` global (the proxy is
/// no longer reachable, so the hidden table no longer reflects it), or if
/// the data cannot be represented as JSON.
pub(crate) fn var_to_json(lua: &Lua) -> Result<Json> {
    let proxy: mlua::Table = lua
        .named_registry_value(VAR_PROXY_REGISTRY)
        .map_err(Error::lua)?;
    let current: Value = lua.globals().get("var").map_err(Error::lua)?;
    match current {
        Value::Table(ref table) if table.equals(&proxy).map_err(Error::lua)? => {}
        _ => {
            return Err(Error::Lua(
                "the `var` global was reassigned; write `var.<field>` instead".to_owned(),
            ));
        }
    }
    let data: mlua::Table = lua
        .named_registry_value(VAR_DATA_REGISTRY)
        .map_err(Error::lua)?;
    let guarded_data: mlua::Table = lua
        .named_registry_value(VAR_GUARDED_DATA_REGISTRY)
        .map_err(Error::lua)?;
    let plain =
        materialize_guarded_value(lua, Value::Table(data), &guarded_data).map_err(Error::lua)?;
    lua.from_value(plain).map_err(Error::lua)
}
