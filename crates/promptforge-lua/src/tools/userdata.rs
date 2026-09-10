//! Inspectable Tool object returned by Lua `tools.bind`.
//!
//! Presentation only: the userdata exposes a bound tool's fields to Lua and
//! serves as the leading handle argument to `tools.call`. Authors read
//! `.name`, `.description`, `.parameters`, `.wire_name`, and `.untrusted`.
//! The object is frozen and methodless (A9): model-facing description
//! overrides are positional arguments to `tools.bind` / `tools.always` /
//! `tools.add`, never assignments on this handle, and invocation is
//! namespace-only through `tools.call(alias_or_tool, arguments)`. Existing
//! callers that ignore the return value keep working.

use mlua::{LuaSerdeExt, MetaMethod, UserData, UserDataFields, UserDataMethods, Value};
use promptforge_tools::{Tool, ToolId};
use serde_json::{Value as Json, json};

/// Inspectable Tool object returned by Lua `tools.bind`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LuaToolHandle {
    name: String,
    description: String,
    parameters: Json,
    wire_name: String,
    untrusted: bool,
}

impl LuaToolHandle {
    /// Builds a handle from a bound alias, capability description, and identity.
    ///
    /// Without a live catalog lookup, `wire_name` is the identity's stable
    /// name, `parameters` is an empty object, and `untrusted` is false.
    #[must_use]
    pub(crate) fn from_binding(
        alias: impl Into<String>,
        description: impl Into<String>,
        id: &ToolId,
    ) -> Self {
        Self {
            name: alias.into(),
            description: description.into(),
            parameters: json!({}),
            wire_name: id.name().to_owned(),
            untrusted: false,
        }
    }

    /// Builds a handle from a live tool and its prompt-local binding metadata.
    pub(crate) fn from_live_binding(
        alias: impl Into<String>,
        description: impl Into<String>,
        tool: &dyn Tool,
    ) -> Self {
        Self {
            name: alias.into(),
            description: description.into(),
            parameters: tool.parameters_schema(),
            wire_name: tool.wire_name().to_owned(),
            // Trust is now carried per-call in `ToolOutput`, not a static
            // per-tool flag; the executor wraps untrusted results at dispatch.
            untrusted: false,
        }
    }

    /// Returns the prompt-local alias.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

impl UserData for LuaToolHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("description", |_, this| Ok(this.description.clone()));
        fields.add_field_method_get("parameters", |lua, this| lua.to_value(&this.parameters));
        fields.add_field_method_get("wire_name", |_, this| Ok(this.wire_name.clone()));
        fields.add_field_method_get("untrusted", |_, this| Ok(this.untrusted));
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
