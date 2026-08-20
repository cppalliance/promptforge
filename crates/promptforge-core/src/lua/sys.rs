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

/// Returns a copy of `sys` with the bound catalog model id under `"model"`.
pub(crate) fn enrich_sys_model(sys: &Json, binding: &ModelBinding) -> Json {
    match sys {
        Json::Object(map) => {
            let mut out = map.clone();
            out.insert(
                "model".to_owned(),
                Json::String(binding.id().name().to_owned()),
            );
            Json::Object(out)
        }
        other => other.clone(),
    }
}

/// Returns a copy of `sys` with `reply_finish_reason` set from the last inference.
pub(crate) fn enrich_sys_reply_finish_reason(sys: &Json, reason: Option<&str>) -> Json {
    match sys {
        Json::Object(map) => {
            let mut out = map.clone();
            out.insert(
                "reply_finish_reason".to_owned(),
                match reason {
                    Some(value) => Json::String(value.to_owned()),
                    None => Json::Null,
                },
            );
            Json::Object(out)
        }
        other => other.clone(),
    }
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
/// then writes through to the data table. `__metatable` is set so author
/// code cannot replace the guard. The data table is stashed in the Lua
/// registry (unreachable from sandboxed author code) and is the read-back
/// source for [`var_to_json`]; the proxy never holds entries itself. The
/// proxy is stashed alongside it so [`var_to_json`] can reject an author
/// reassigning the `var` global instead of silently reading stale data.
pub(crate) fn guarded_var(lua: &Lua, initial: Option<&Json>) -> Result<mlua::Table> {
    let data = match initial {
        Some(value) => match lua.to_value(value).map_err(Error::lua)? {
            Value::Table(table) => table,
            other => {
                return Err(Error::Lua(format!(
                    "var must seed from a JSON object, got {}",
                    other.type_name()
                )));
            }
        },
        None => lua.create_table().map_err(Error::lua)?,
    };

    let proxy = lua.create_table().map_err(Error::lua)?;
    let metatable = lua.create_table().map_err(Error::lua)?;
    metatable.set("__index", data.clone()).map_err(Error::lua)?;

    let write_data = data.clone();
    let newindex = lua
        .create_function(
            move |lua, (_proxy, key, value): (Value, Value, Value)| -> mlua::Result<()> {
                if let Value::Function(_) | Value::UserData(_) | Value::Thread(_) = value {
                    let field = match &key {
                        Value::String(name) => name.to_string_lossy(),
                        other => format!("{other:?}"),
                    };
                    return Err(mlua::Error::runtime(format!(
                        "var.{field} must be JSON data, got {}",
                        value.type_name()
                    )));
                }
                // Deep check: the serde bridge walks nested tables, so a
                // function, userdata, or thread anywhere in the value fails
                // here, at the assigning line, rather than at read-back.
                lua.from_value::<Json>(value.clone())?;
                write_data.raw_set(key, value)
            },
        )
        .map_err(Error::lua)?;
    metatable.set("__newindex", newindex).map_err(Error::lua)?;
    metatable
        .set("__metatable", "var is guarded")
        .map_err(Error::lua)?;

    proxy.set_metatable(Some(metatable));
    // The named registry entries keep the data table alive and reachable for
    // host read-back, and the proxy reachable for the reassignment check in
    // `var_to_json`.
    lua.set_named_registry_value(VAR_DATA_REGISTRY, data)
        .map_err(Error::lua)?;
    lua.set_named_registry_value(VAR_PROXY_REGISTRY, proxy.clone())
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
    lua.from_value(Value::Table(data)).map_err(Error::lua)
}
