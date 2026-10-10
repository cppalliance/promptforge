//! The tool object: one catalog tool as Lua sees it.
//!
//! Presentation only: the userdata exposes an offered tool's `id` and its
//! catalog `description`, and stands for its id wherever `tools.offer`,
//! `tools.always_offer`, and `tools.call` take a tool. The object is frozen
//! and methodless: model-facing description overrides are positional
//! arguments to `tools.offer` / `tools.always_offer`, never assignments on
//! this handle, so `description` stays the catalog text whatever a section
//! overrides. Unlike a model handle, which carries `infer` and `loop`, it
//! is invoked only through `tools.call(tool, arguments)`.

use mlua::{MetaMethod, UserData, UserDataFields, UserDataMethods, Value};
use promptforge_types::tools::ToolId;

use crate::handles::ToolBinding;

/// One catalog tool as Lua sees it: `id` and `description`, both
/// read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaToolHandle {
    id: ToolId,
    description: String,
}

impl LuaToolHandle {
    /// Builds the object for an offered tool from its binding: the id and
    /// the catalog description, never the binding's override.
    #[must_use]
    pub(crate) fn from_binding(binding: &ToolBinding) -> Self {
        Self {
            id: binding.id().clone(),
            description: binding.description().to_owned(),
        }
    }

    /// Returns the tool's canonical id, which the object stands for.
    #[must_use]
    pub(crate) fn id(&self) -> &ToolId {
        &self.id
    }
}

impl UserData for LuaToolHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("id", |_, this| Ok(this.id.to_string()));
        fields.add_field_method_get("description", |_, this| Ok(this.description.clone()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, _, (key, _): (String, Value)| -> mlua::Result<()> {
                Err(mlua::Error::external(format!(
                    "Tool objects are frozen: cannot assign field {key:?}"
                )))
            },
        );
    }
}
