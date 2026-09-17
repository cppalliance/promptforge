//! The `argv` global: the parsed form of the run's args string.
//!
//! `argv` installs at host injection in one of two modes. The H1 pass gets
//! a plain writable value, so the repair pattern lives there: read the
//! broken input from `args`, assign `argv = repaired`, and the executor
//! reads the value back when the pass completes. Every other section gets
//! the frozen value: reads work (absent fields read nil), and any
//! assignment - `argv = ...` or `argv.field = ...` at any depth - raises.
//!
//! The freeze rides on the `_G` metatable, the same composition the lazy
//! `prose` guard uses: `argv` is never a raw global in a frozen section, so
//! every read and every write of the name crosses the guard, and every
//! other key delegates to whatever metatable was installed first (the
//! `prose` guard installs later and shadows this pair as its delegates, so
//! the two compose). The table value itself is deep-frozen behind proxy
//! tables whose `__newindex` rejects every write.

use super::{Error, Json, Lua, LuaSerdeExt, MultiValue, Result, Value};

/// How a section VM installs the `argv` global at host injection. `None`
/// installs nil either way, so `if argv then` is the idiomatic malformed
/// check.
#[derive(Debug, Clone, Copy)]
pub enum Argv<'a> {
    /// The H1 pass: a plain writable value, so the repair pattern can
    /// assign `argv`; the executor reads the value back at the freeze.
    Writable(Option<&'a Json>),
    /// Every other section: reads work (absent fields read nil), and every
    /// assignment - `argv = ...` or a field write at any depth - raises.
    Frozen(Option<&'a Json>),
}

/// Marker field on a metatable this module installed: a re-install reuses
/// the recorded delegates instead of chaining a new handler over its own.
const GUARD_MARKER: &str = "__promptforge_argv_guard";
/// The metatable field recording the `__index` the guard shadows.
const DELEGATE_INDEX: &str = "__promptforge_argv_delegate_index";
/// The metatable field recording the `__newindex` the guard shadows.
const DELEGATE_NEWINDEX: &str = "__promptforge_argv_delegate_newindex";

/// Installs `argv` as a plain writable global: the H1 pass's mode. `None`
/// (malformed args, or JSON null) installs nil, so `if argv then` is the
/// idiomatic malformed check.
///
/// # Errors
/// Returns [`Error::Lua`] if the value cannot be bridged or installed.
pub(crate) fn install_writable(lua: &Lua, argv: Option<&Json>) -> Result<()> {
    let value = match argv {
        None | Some(Json::Null) => Value::Nil,
        Some(json) => lua.to_value(json).map_err(Error::lua)?,
    };
    lua.globals().raw_set("argv", value).map_err(Error::lua)
}

/// Installs `argv` frozen: reads work, every assignment raises. This is the
/// mode of every section but H1 - the value H1 left behind at the freeze.
///
/// # Errors
/// Returns [`Error::Lua`] if the value cannot be bridged or the guard
/// metatable cannot be built or installed.
pub(crate) fn install_frozen(lua: &Lua, argv: Option<&Json>) -> Result<()> {
    let frozen = frozen_json_value(lua, argv)?;
    let globals = lua.globals();
    let old = globals.metatable();
    // The delegates the new guard shadows: a metatable of our own already
    // recorded its delegates, so a re-install reuses them rather than
    // chaining over the previous handler; any other metatable (the prose
    // guard's, a shared library's) contributes its own index pair.
    let (delegate_index, delegate_newindex) = match &old {
        Some(old) if matches!(old.raw_get::<Value>(GUARD_MARKER), Ok(Value::Boolean(true))) => (
            old.raw_get::<Value>(DELEGATE_INDEX).map_err(Error::lua)?,
            old.raw_get::<Value>(DELEGATE_NEWINDEX)
                .map_err(Error::lua)?,
        ),
        Some(old) => (
            old.raw_get::<Value>("__index").map_err(Error::lua)?,
            old.raw_get::<Value>("__newindex").map_err(Error::lua)?,
        ),
        None => (Value::Nil, Value::Nil),
    };
    let metatable = lua.create_table().map_err(Error::lua)?;
    // Carry every other field the previous metatable installed, then shadow
    // the index pair with the argv guard.
    if let Some(old) = &old {
        for pair in old.clone().pairs::<Value, Value>() {
            let (key, value) = pair.map_err(Error::lua)?;
            let shadowed =
                matches!(&key, Value::String(name) if name == "__index" || name == "__newindex");
            if !shadowed {
                metatable.raw_set(key, value).map_err(Error::lua)?;
            }
        }
    }
    metatable.raw_set(GUARD_MARKER, true).map_err(Error::lua)?;
    metatable
        .raw_set(DELEGATE_INDEX, delegate_index.clone())
        .map_err(Error::lua)?;
    metatable
        .raw_set(DELEGATE_NEWINDEX, delegate_newindex.clone())
        .map_err(Error::lua)?;

    let index = lua
        .create_function(move |_, (target, key): (mlua::Table, Value)| {
            if matches!(&key, Value::String(name) if name == "argv") {
                return Ok(frozen.clone());
            }
            match &delegate_index {
                Value::Function(function) => Ok(function
                    .call::<MultiValue>((target, key))?
                    .into_iter()
                    .next()
                    .unwrap_or(Value::Nil)),
                Value::Table(table) => table.get(key),
                _ => Ok(Value::Nil),
            }
        })
        .map_err(Error::lua)?;
    metatable.raw_set("__index", index).map_err(Error::lua)?;

    let newindex = lua
        .create_function(
            move |_, (target, key, value): (mlua::Table, Value, Value)| -> mlua::Result<()> {
                if matches!(&key, Value::String(name) if name == "argv") {
                    return Err(mlua::Error::runtime(
                        "argv is frozen outside H1: assign it in H1 only",
                    ));
                }
                match &delegate_newindex {
                    Value::Function(function) => {
                        function.call::<MultiValue>((target, key, value))?;
                        Ok(())
                    }
                    Value::Table(table) => table.set(key, value),
                    _ => target.raw_set(key, value),
                }
            },
        )
        .map_err(Error::lua)?;
    metatable
        .raw_set("__newindex", newindex)
        .map_err(Error::lua)?;
    globals.set_metatable(Some(metatable)).map_err(Error::lua)
}

/// Builds the frozen Lua form of an argv JSON value: tables become
/// deep-frozen proxies, scalars bridge directly, and absent or null reads
/// as nil.
fn frozen_json_value(lua: &Lua, value: Option<&Json>) -> Result<Value> {
    match value {
        None | Some(Json::Null) => Ok(Value::Nil),
        Some(Json::Array(values)) => {
            let data = lua
                .create_table_with_capacity(values.len(), 0)
                .map_err(Error::lua)?;
            for (index, value) in values.iter().enumerate() {
                data.raw_set(index + 1, frozen_json_value(lua, Some(value))?)
                    .map_err(Error::lua)?;
            }
            freeze_table(lua, data).map(Value::Table)
        }
        Some(Json::Object(values)) => {
            let data = lua
                .create_table_with_capacity(0, values.len())
                .map_err(Error::lua)?;
            for (key, value) in values {
                data.raw_set(key.as_str(), frozen_json_value(lua, Some(value))?)
                    .map_err(Error::lua)?;
            }
            freeze_table(lua, data).map(Value::Table)
        }
        Some(scalar) => lua.to_value(scalar).map_err(Error::lua),
    }
}

/// Wraps a plain data table in a frozen proxy: reads pass through `__index`
/// to the data (whose nested tables are already frozen proxies, and whose
/// absent keys read nil), and every write raises the freeze error.
fn freeze_table(lua: &Lua, data: mlua::Table) -> Result<mlua::Table> {
    let proxy = lua.create_table().map_err(Error::lua)?;
    let metatable = lua.create_table().map_err(Error::lua)?;
    metatable.raw_set("__index", data).map_err(Error::lua)?;
    let newindex = lua
        .create_function(
            |_, (_proxy, key, _value): (Value, Value, Value)| -> mlua::Result<()> {
                let field = match &key {
                    Value::String(name) => format!("'{}'", name.to_string_lossy()),
                    other => format!("{other:?}"),
                };
                Err(mlua::Error::runtime(format!(
                    "argv is frozen outside H1: cannot set field {field}"
                )))
            },
        )
        .map_err(Error::lua)?;
    metatable
        .raw_set("__newindex", newindex)
        .map_err(Error::lua)?;
    metatable
        .raw_set("__metatable", "argv is frozen")
        .map_err(Error::lua)?;
    proxy.set_metatable(Some(metatable)).map_err(Error::lua)?;
    Ok(proxy)
}
