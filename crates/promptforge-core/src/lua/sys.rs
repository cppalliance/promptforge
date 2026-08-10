use super::{Error, Json, Lua, LuaSerdeExt, ModelBinding, Result, Value};

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

    let proxy = lua
        .create_table()
        .map_err(Error::lua)?;
    let metatable = lua
        .create_table()
        .map_err(Error::lua)?;

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
    metatable
        .set("__index", index)
        .map_err(Error::lua)?;

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
    metatable
        .set("__newindex", newindex)
        .map_err(Error::lua)?;
    metatable
        .set("__metatable", "sys is sealed")
        .map_err(Error::lua)?;

    proxy.set_metatable(Some(metatable));
    Ok(proxy)
}
