//! Capability preludes: the Lua source an activated capability contributes,
//! installed into every section VM before the shared library replays.
//!
//! A prelude runs once per VM as a main chunk, in an environment table of
//! its own. That table's metatable `__index` is a lookup table of the base
//! functions that survive hardening, the `string`, `table`, and `math`
//! libraries, `tools`, `store`, `untrusted`, and a view of `var` that is
//! read-only at every depth, each read from `_G` when the prelude installs. Reading them then means
//! `pcall` and `xpcall` are the coroutine shim's normalized versions and
//! `tools.call` is its yield shim, and because the store yield shims later
//! land in the same `store` table, a prelude function called from a block
//! reaches them too. Every other global, another prelude's included, reads
//! as nil. Functions the prelude defines keep this environment when author
//! code calls them later.
//!
//! The environment has no `__newindex`, so every global the prelude assigns
//! lands in the environment table itself, and once the chunk returns, that
//! table's own entries are exactly the prelude's globals. Each one is
//! checked for a collision, a table is sealed at its top level the way
//! `sys` is, and each is raw-set into `_G`.

use mlua::Table;
use mlua::chunk::ChunkMode;
use promptforge_types::plugins::{PluginId, Prelude};

use super::{BTreeMap, Error, Lua, Result, Value};
use crate::proxy::read_only_proxy;

/// The `_G` names a prelude's environment reads, as bound when it installs.
const VISIBLE_GLOBALS: [&str; 19] = [
    "assert",
    "error",
    "getmetatable",
    "ipairs",
    "next",
    "pairs",
    "pcall",
    "select",
    "setmetatable",
    "tonumber",
    "tostring",
    "type",
    "xpcall",
    "string",
    "table",
    "math",
    "tools",
    "store",
    "untrusted",
];

/// Installs the run's capability preludes, in order, into a section VM.
///
/// Each prelude loads from source under the chunk name
/// `@plugin:<id>` in its own restricted environment (see the module
/// docs). The globals it defines must not collide with a reserved name
/// ([`crate::RESERVED_NAMES`]), with any other name bound in `_G`, with
/// `aliases` (the prompt's frontmatter tool and model aliases, which
/// install as globals after the shared replay), or with an earlier
/// prelude's globals. A table global installs as an empty proxy that reads
/// the prelude's table and refuses writes; any other value installs as it
/// is.
///
/// The Engine's section setup calls this after the coroutine yield shims
/// install and before the shared library replays. The chunk is not a
/// coroutine, so a prelude that calls `tools.call` while loading fails.
///
/// # Errors
/// Returns [`Error::LuaRuntime`] naming the capability when a prelude
/// fails to load, and [`Error::Lua`] naming the capability, the global,
/// and what it collides with when a global collides or is not named by a
/// string.
pub fn install_preludes(lua: &Lua, preludes: &[Prelude], aliases: &[&str]) -> Result<()> {
    let globals = lua.globals();
    let mut installed: BTreeMap<String, &PluginId> = BTreeMap::new();
    for prelude in preludes {
        let plugin = prelude.plugin();
        let env = environment(lua, &globals)?;
        lua.load(prelude.source())
            .set_name(format!("@plugin:{plugin}"))
            .set_mode(ChunkMode::Text)
            .set_environment(env.clone())
            .exec()
            .map_err(|error| load_failure(plugin, error))?;
        let defined = defined_globals(plugin, &env)?;
        for name in defined.keys() {
            check_collision(&globals, plugin, name, &installed, aliases)?;
        }
        for (name, value) in defined {
            let value = match value {
                Value::Table(table) => Value::Table(seal(lua, plugin, &name, table)?),
                other => other,
            };
            globals.raw_set(name.as_str(), value).map_err(Error::lua)?;
            installed.insert(name, plugin);
        }
    }
    Ok(())
}

/// Builds one prelude's environment. Each prelude gets its own
/// environment and lookup table, so no prelude sees another prelude's
/// globals, and the `string`, `table`, `math`, `tools`, and `store` tables
/// it reads through that lookup are shared with the rest of the VM.
fn environment(lua: &Lua, globals: &Table) -> Result<Table> {
    let lookup = lua.create_table().map_err(Error::lua)?;
    for name in VISIBLE_GLOBALS {
        let value: Value = globals.raw_get(name).map_err(Error::lua)?;
        lookup.raw_set(name, value).map_err(Error::lua)?;
    }
    if let Value::Table(var) = globals.raw_get::<Value>("var").map_err(Error::lua)? {
        let view = read_only_var(lua, var, "var".to_owned()).map_err(Error::lua)?;
        lookup.raw_set("var", view).map_err(Error::lua)?;
    }
    let metatable = lua.create_table().map_err(Error::lua)?;
    metatable.raw_set("__index", lookup).map_err(Error::lua)?;
    let env = lua.create_table().map_err(Error::lua)?;
    env.set_metatable(Some(metatable)).map_err(Error::lua)?;
    Ok(env)
}

/// Builds a read-only view of a guarded `var` table at `path`: a read-only
/// proxy whose `__index` reads `target` and hands back each table it finds
/// as a view of its own, so no depth of `var` is writable through it.
fn read_only_var(lua: &Lua, target: Table, path: String) -> mlua::Result<Table> {
    let refusal = format!("{path} is read-only inside a Plugin prelude; cannot set");
    let index = lua.create_function(move |lua, (_view, key): (Value, Value)| {
        match target.get::<Value>(key.clone())? {
            Value::Table(nested) => {
                read_only_var(lua, nested, child_path(&path, &key)).map(Value::Table)
            }
            other => Ok(other),
        }
    })?;
    read_only_proxy(
        lua,
        Value::Function(index),
        move |key| format!("{refusal} '{}'", field_name(key)),
        "var is read-only",
    )
}

/// Renders the path of a field read through a `var` view, for its refusal.
fn child_path(path: &str, key: &Value) -> String {
    match key {
        Value::String(name) => format!("{path}.{}", name.to_string_lossy()),
        Value::Integer(index) => format!("{path}[{index}]"),
        other => format!("{path}[{other:?}]"),
    }
}

/// Reads the prelude's globals off its environment table's own entries,
/// sorted by name so the checks and the install run in a fixed order.
fn defined_globals(plugin: &PluginId, env: &Table) -> Result<BTreeMap<String, Value>> {
    let mut defined = BTreeMap::new();
    for pair in env.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(Error::lua)?;
        if let Value::String(name) = &key
            && let Ok(name) = name.to_str()
        {
            defined.insert(name.to_string(), value);
            continue;
        }
        let key = match &key {
            Value::String(_) => "a string key that is not valid UTF-8".to_owned(),
            other => format!("a key of type {}", other.type_name()),
        };
        return Err(Error::Lua(format!(
            "Plugin `{plugin}`: its prelude defines a global under {key}; a global's \
             name must be a UTF-8 string"
        )));
    }
    Ok(defined)
}

/// Fails when `name` is already taken: by an earlier prelude, a
/// frontmatter alias, a reserved name, or a name bound in `_G`.
///
/// The reserved check covers the Engine globals that a raw `_G` read does
/// not find on every VM (`ui` and `item` bind only on some, and the `_G`
/// guard serves `argv` outside H1 and `prose`), so whether a prelude
/// installs does not depend on the Host or the section.
fn check_collision(
    globals: &Table,
    plugin: &PluginId,
    name: &str,
    installed: &BTreeMap<String, &PluginId>,
    aliases: &[&str],
) -> Result<()> {
    let collides_with = if let Some(owner) = installed.get(name) {
        format!("which Plugin `{owner}`'s prelude already defines")
    } else if aliases.contains(&name) {
        "which the prompt's frontmatter binds as a tool or model alias".to_owned()
    } else if let Some(kind) = crate::reserved_name(name) {
        format!("which is reserved as {kind}")
    } else if !matches!(
        globals.raw_get::<Value>(name).map_err(Error::lua)?,
        Value::Nil
    ) {
        "which is already an Engine global".to_owned()
    } else {
        return Ok(());
    };
    Err(Error::Lua(format!(
        "Plugin `{plugin}`: its prelude defines the global `{name}`, {collides_with}"
    )))
}

/// Seals a prelude's table global at its top level: an empty proxy whose
/// `__index` reads the hidden table, whose `__newindex` raises, and whose
/// `__metatable` is set, so `pairs` over it sees nothing.
fn seal(lua: &Lua, plugin: &PluginId, name: &str, hidden: Table) -> Result<Table> {
    let refusal = format!("{name} is read-only: Plugin `{plugin}` defines it; cannot set");
    read_only_proxy(
        lua,
        Value::Table(hidden),
        move |key| format!("{refusal} '{}'", field_name(key)),
        &format!("{name} is sealed"),
    )
    .map_err(Error::lua)
}

/// Renders a refused field's key for an error message.
fn field_name(key: &Value) -> String {
    match key {
        Value::String(name) => name.to_string_lossy(),
        other => format!("{other:?}"),
    }
}

/// Maps a prelude's load or run failure to the model-facing message,
/// keeping the Lua traceback after it and the `mlua` error as the cause.
fn load_failure(plugin: &PluginId, error: mlua::Error) -> Error {
    let rendered = match &error {
        mlua::Error::RuntimeError(message) => message.clone(),
        other => other.to_string(),
    };
    let (head, traceback) = match rendered.split_once("\nstack traceback:") {
        Some((head, traceback)) => (head, Some(traceback)),
        None => (rendered.as_str(), None),
    };
    let mut message = format!(
        "Plugin `{plugin}`: its prelude failed to load: {head}. A prelude only \
         defines functions; it must not call tools while loading."
    );
    if let Some(traceback) = traceback {
        message.push_str("\nstack traceback:");
        message.push_str(traceback);
    }
    Error::LuaRuntime {
        message,
        source: Box::new(error),
    }
}

#[cfg(test)]
#[path = "prelude-tests.rs"]
mod tests;
