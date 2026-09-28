//! The read-only proxy every seal builds on: `sys`, a frozen `argv` table,
//! a sealed prelude global, and a prelude's `var` view.

use mlua::{MaybeSend, Table};

use super::{Lua, Value};

/// Builds an empty proxy whose `__index` is `index`, whose `__newindex`
/// raises the text `refusal` builds from the refused key, and whose
/// `__metatable` is `label`, so `pairs` over it sees nothing and
/// `getmetatable` returns only the label.
pub(crate) fn read_only_proxy(
    lua: &Lua,
    index: Value,
    refusal: impl Fn(&Value) -> String + MaybeSend + 'static,
    label: &str,
) -> mlua::Result<Table> {
    let newindex = lua.create_function(
        move |_lua, (_proxy, key, _value): (Value, Value, Value)| -> mlua::Result<()> {
            Err(mlua::Error::runtime(refusal(&key)))
        },
    )?;
    let metatable = lua.create_table()?;
    metatable.raw_set("__index", index)?;
    metatable.raw_set("__newindex", newindex)?;
    metatable.raw_set("__metatable", label)?;
    let proxy = lua.create_table()?;
    proxy.set_metatable(Some(metatable))?;
    Ok(proxy)
}
