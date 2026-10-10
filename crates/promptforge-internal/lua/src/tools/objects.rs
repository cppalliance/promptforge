//! Tool objects: one frozen [`LuaToolHandle`] per offered tool, built once
//! per VM, and `tools.required`, `tools.extras`, and `tools.get` over them.
//!
//! Every list hands out the same userdata for a tool, so a tool read
//! through any of them compares `==` to the same tool read through
//! another, while each list call builds a fresh array.

use std::sync::{Arc, Mutex};

use mlua::{AnyUserData, Lua, Table, Value};

use super::lock_tools;
use super::userdata::LuaToolHandle;
use crate::error::{Error, Result};
use crate::handles::ToolSet;

/// The registry key of the VM's tool-object table: one object per offered
/// tool, keyed by id, built when `install_tools` runs.
const TOOL_OBJECTS_REGISTRY: &str = "promptforge.tools.objects";

/// The VM's tool object for the catalog id `id`, or `None` when the run
/// cannot offer that tool.
///
/// # Errors
/// Returns an `mlua` error when the registry cannot be read.
pub(crate) fn tool_object(lua: &Lua, id: &str) -> mlua::Result<Option<AnyUserData>> {
    let objects: Option<Table> = lua.named_registry_value(TOOL_OBJECTS_REGISTRY)?;
    match objects
        .map(|objects| objects.raw_get::<Value>(id))
        .transpose()?
    {
        Some(Value::UserData(object)) => Ok(Some(object)),
        _ => Ok(None),
    }
}

/// Builds the VM's tool-object table over the set's offering.
///
/// # Errors
/// Returns [`Error::Lua`] if an object or the table cannot be created or
/// stored.
pub(super) fn install_tool_objects(lua: &Lua, set: &ToolSet) -> Result<()> {
    let objects = lua.create_table().map_err(Error::lua)?;
    for binding in set.offered() {
        let object = lua
            .create_userdata(LuaToolHandle::from_binding(binding))
            .map_err(Error::lua)?;
        objects
            .raw_set(binding.id().to_string(), object)
            .map_err(Error::lua)?;
    }
    lua.set_named_registry_value(TOOL_OBJECTS_REGISTRY, objects)
        .map_err(Error::lua)
}

/// Installs `tools.required()` and `tools.extras()`, the offered tools
/// whose Plugin the frontmatter declares and every other one, and
/// `tools.get(id)`.
///
/// # Errors
/// Returns [`Error::Lua`] if a function cannot be created or installed.
pub(super) fn install_tool_lists(
    lua: &Lua,
    tools: &Table,
    set: &Arc<Mutex<ToolSet>>,
) -> Result<()> {
    for (name, required) in [("required", true), ("extras", false)] {
        let shared = Arc::clone(set);
        let list = lua
            .create_function(move |lua, ()| {
                let ids: Vec<String> = {
                    let set = lock_tools(&shared)?;
                    set.offered()
                        .iter()
                        .filter(|binding| {
                            set.declared().contains(&binding.id().plugin()) == required
                        })
                        .map(|binding| binding.id().to_string())
                        .collect()
                };
                let objects = lua.create_table()?;
                for id in ids {
                    if let Some(object) = tool_object(lua, &id)? {
                        objects.raw_push(object)?;
                    }
                }
                Ok(objects)
            })
            .map_err(Error::lua)?;
        tools.set(name, list).map_err(Error::lua)?;
    }
    let get = lua
        .create_function(|lua, id: Value| match id {
            Value::String(id) => tool_object(lua, &id.to_str()?),
            other => Err(mlua::Error::external(format!(
                "tools.get takes a tool id, got {}",
                other.type_name()
            ))),
        })
        .map_err(Error::lua)?;
    tools.set("get", get).map_err(Error::lua)
}
