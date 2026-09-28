//! The `argv` global: the parsed form of the run's args string.
//!
//! `argv` installs at host injection in one of two modes. The H1 pass gets
//! a plain writable value, so the repair pattern lives there: read the
//! broken input from `args`, assign `argv = repaired`, and the executor
//! reads the value back when the pass completes. Every other section gets
//! the frozen value: reads work (absent fields read nil), and any
//! assignment - `argv = ...` or `argv.field = ...` at any depth - raises.
//!
//! The freeze sits on the `_G` guard ([`crate::globals`]): `argv` is never
//! a raw global in a frozen section, so every read and every write of the
//! name crosses the guard, which serves the frozen value and refuses the
//! assignment before any metatable author code set on `_G` sees the key.
//! The table value itself is deep-frozen behind proxy tables whose
//! `__newindex` rejects every write.

use super::{Error, Json, Lua, LuaSerdeExt, Result, Value};
use crate::proxy::read_only_proxy;

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

/// The refusal an assignment of the frozen `argv` global raises.
pub(crate) const ASSIGNMENT_REFUSAL: &str = "argv is frozen outside H1: assign it in H1 only";

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
/// cannot record it.
pub(crate) fn install_frozen(lua: &Lua, argv: Option<&Json>) -> Result<()> {
    let frozen = frozen_json_value(lua, argv)?;
    crate::globals::freeze_argv(lua, frozen)
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
    read_only_proxy(
        lua,
        Value::Table(data),
        |key| {
            let field = match key {
                Value::String(name) => format!("'{}'", name.to_string_lossy()),
                other => format!("{other:?}"),
            };
            format!("argv is frozen outside H1: cannot set field {field}")
        },
        "argv is frozen",
    )
    .map_err(Error::lua)
}
