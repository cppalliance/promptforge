//! The `plugins` table: one read-only object per Plugin the run knows.
//!
//! A Plugin the frontmatter declares has an object even when it offers no
//! tool, and every other Plugin has one when it offers a tool. The objects
//! are built once per VM, after the tool objects, so `plugins.required()`,
//! `plugins.extras()`, `plugins.get`, and a tool object's `plugin` getter
//! hand out the same userdata for a Plugin, while each list call builds a
//! fresh array. A Plugin object's `tools` reads the shared tool objects,
//! so it passes straight to `tools.offer`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, MetaMethod, Table, UserData, UserDataFields, UserDataMethods, Value};
use promptforge_types::plugins::PluginId;

use crate::error::{Error, Result};
use crate::handles::ToolSet;
use crate::tools::tool_object;

/// The registry key of the VM's plugin-object table, keyed by name.
const PLUGIN_OBJECTS_REGISTRY: &str = "promptforge.plugins.objects";

/// One Plugin as Lua sees it: `name`, and `tools`, which reads a fresh
/// array of the Plugin's tool objects in id order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaPluginHandle {
    name: PluginId,
    /// The ids of the Plugin's offered tools, in id order.
    tools: Vec<String>,
}

impl UserData for LuaPluginHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.to_string()));
        fields.add_field_method_get("tools", |lua, this| {
            let objects = lua.create_table()?;
            for id in &this.tools {
                if let Some(object) = tool_object(lua, id)? {
                    objects.raw_push(object)?;
                }
            }
            Ok(objects)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, _, (key, _): (String, Value)| -> mlua::Result<()> {
                Err(mlua::Error::external(format!(
                    "Plugin objects are frozen: cannot assign field {key:?}"
                )))
            },
        );
    }
}

/// The VM's plugin object for `name`, read by `plugins.get` and by a tool
/// object's `plugin` getter, or `None` for a Plugin the run does not know.
///
/// # Errors
/// Returns an `mlua` error when the registry cannot be read.
pub(crate) fn plugin_object(lua: &Lua, name: &str) -> mlua::Result<Option<AnyUserData>> {
    let objects: Option<Table> = lua.named_registry_value(PLUGIN_OBJECTS_REGISTRY)?;
    match objects
        .map(|objects| objects.raw_get::<Value>(name))
        .transpose()?
    {
        Some(Value::UserData(object)) => Ok(Some(object)),
        _ => Ok(None),
    }
}

/// Builds one plugin object per declared Plugin and per other Plugin with
/// offered tools, then installs `plugins.required()`, the declared
/// Plugins, `plugins.extras()`, the others, both in name order, and
/// `plugins.get(name)`.
///
/// # Errors
/// Returns [`Error::Lua`] if an object or function cannot be created or installed.
pub(crate) fn install_plugins(lua: &Lua, globals: &Table, set: &Arc<Mutex<ToolSet>>) -> Result<()> {
    let (plugins, declared) = {
        let set = set
            .lock()
            .map_err(|_| Error::Lua("tool set mutex was poisoned".to_owned()))?;
        (plugin_tools(&set), set.declared().to_vec())
    };
    let objects = lua.create_table().map_err(Error::lua)?;
    let (mut required, mut extras) = (Vec::new(), Vec::new());
    for (name, tools) in plugins {
        let key = name.to_string();
        if declared.contains(&name) {
            required.push(key.clone());
        } else {
            extras.push(key.clone());
        }
        let object = lua
            .create_userdata(LuaPluginHandle { name, tools })
            .map_err(Error::lua)?;
        objects.raw_set(key, object).map_err(Error::lua)?;
    }
    lua.set_named_registry_value(PLUGIN_OBJECTS_REGISTRY, objects)
        .map_err(Error::lua)?;

    let table = lua.create_table().map_err(Error::lua)?;
    for (function, names) in [("required", required), ("extras", extras)] {
        let list = lua
            .create_function(move |lua, ()| {
                let objects = lua.create_table()?;
                for name in &names {
                    if let Some(object) = plugin_object(lua, name)? {
                        objects.raw_push(object)?;
                    }
                }
                Ok(objects)
            })
            .map_err(Error::lua)?;
        table.set(function, list).map_err(Error::lua)?;
    }
    let get = lua
        .create_function(|lua, name: Value| match name {
            Value::String(name) => plugin_object(lua, &name.to_str()?),
            other => Err(mlua::Error::external(format!(
                "plugins.get takes a Plugin name, got {}",
                other.type_name()
            ))),
        })
        .map_err(Error::lua)?;
    table.set("get", get).map_err(Error::lua)?;
    globals.raw_set("plugins", table).map_err(Error::lua)
}

/// Each Plugin the run knows, in name order, with the ids of its offered
/// tools in the set's id order: every declared Plugin, with or without
/// tools, and every other Plugin that offers one.
fn plugin_tools(set: &ToolSet) -> BTreeMap<PluginId, Vec<String>> {
    let mut plugins: BTreeMap<PluginId, Vec<String>> = set
        .declared()
        .iter()
        .map(|name| (name.clone(), Vec::new()))
        .collect();
    for binding in set.offered() {
        plugins
            .entry(binding.id().plugin())
            .or_default()
            .push(binding.id().to_string());
    }
    plugins
}

#[cfg(test)]
#[path = "plugins-tests.rs"]
mod tests;
