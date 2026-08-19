use super::{
    Json, LuaSerdeExt, MetaMethod, Result, ToolId, UserData, UserDataFields, UserDataMethods,
    Value, json,
};

/// Resolves one plain-English capability description to one stable live tool.
///
/// This is the deterministic seam used by live H1 resolution. It keeps core
/// independent of any concrete picker implementation while allowing a caller
/// to supply a fixed resolver in tests.
pub(crate) trait ToolResolver: Send + Sync {
    /// Resolves `description` to a stable tool identity.
    ///
    /// # Errors
    /// Returns a core error when the capability cannot be resolved uniquely.
    fn resolve(&self, description: &str) -> Result<ToolId>;
}

impl<F> ToolResolver for F
where
    F: Fn(&str) -> Result<ToolId> + Send + Sync,
{
    fn resolve(&self, description: &str) -> Result<ToolId> {
        self(description)
    }
}

/// One prompt-local alias bound to one stable live tool identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolBinding {
    pub(crate) alias: String,
    pub(crate) description: String,
    pub(crate) id: ToolId,
    /// Author override for the model-facing schema description.
    ///
    /// Capability text in [`Self::description`] stays the live H1 need
    /// string. When set, [`crate::execute`] advertises this instead of the
    /// registry tool's default description.
    pub(crate) model_description: Option<String>,
}

impl ToolBinding {
    #[cfg(test)]
    pub(crate) fn for_test(alias: &str, description: &str, id: ToolId) -> Self {
        Self {
            alias: alias.to_owned(),
            description: description.to_owned(),
            id,
            model_description: None,
        }
    }

    /// Returns the exact prompt-local alias.
    #[must_use]
    pub(crate) fn alias(&self) -> &str {
        &self.alias
    }

    /// Returns the declared capability description.
    #[must_use]
    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    /// Returns the selected stable live identity.
    #[must_use]
    pub(crate) fn id(&self) -> &ToolId {
        &self.id
    }

    /// Returns the author override for the model-facing description, if any.
    #[must_use]
    pub(crate) fn model_description(&self) -> Option<&str> {
        self.model_description.as_deref()
    }
}

/// Inspectable Tool object returned by Lua `tools.need`.
///
/// Authors read `.name`, `.description`, `.parameters`, `.wire_name`, and
/// `.untrusted`. `.description` is mutable: assigning it before `tools.add`
/// overrides the model-facing schema text. Existing callers that ignore the
/// return value keep working.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LuaToolHandle {
    name: String,
    description: String,
    description_overridden: bool,
    parameters: Json,
    wire_name: String,
    untrusted: bool,
}

impl LuaToolHandle {
    /// Builds a handle from a bound alias, capability description, and identity.
    ///
    /// Without a live registry lookup, `wire_name` is the identity's stable
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
            description_overridden: false,
            parameters: json!({}),
            wire_name: id.name().to_owned(),
            untrusted: false,
        }
    }

    pub(crate) fn from_live_binding(
        alias: impl Into<String>,
        description: impl Into<String>,
        tool: &dyn crate::tools::Tool,
    ) -> Self {
        Self {
            name: alias.into(),
            description: description.into(),
            description_overridden: false,
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

    /// Returns the model-facing description override when the author assigned
    /// `.description` on this handle.
    #[must_use]
    pub(crate) fn model_description_override(&self) -> Option<&str> {
        self.description_overridden
            .then_some(self.description.as_str())
    }
}

impl UserData for LuaToolHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("description", |_, this| Ok(this.description.clone()));
        fields.add_field_method_set("description", |_, this, value: String| {
            this.description = value;
            this.description_overridden = true;
            Ok(())
        });
        fields.add_field_method_get("parameters", |lua, this| lua.to_value(&this.parameters));
        fields.add_field_method_get("wire_name", |_, this| Ok(this.wire_name.clone()));
        fields.add_field_method_get("untrusted", |_, this| Ok(this.untrusted));
    }
}

/// Inspectable Section object from the Lua `tasks` table.
///
/// Authors read `.name` and `.has_prose`, and pass the object to `execute` or
/// `jump` in place of a heading string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LuaSectionHandle {
    name: String,
    heading: String,
    has_prose: bool,
}

impl LuaSectionHandle {
    /// Builds a handle for a top-level H2 section.
    #[must_use]
    pub(crate) fn new(name: impl Into<String>, has_prose: bool) -> Self {
        let name = name.into();
        let heading = format!("## {name}");
        Self {
            name,
            heading,
            has_prose,
        }
    }

    /// Returns the canonical `"## Name"` heading used by `execute` / `jump`.
    #[must_use]
    pub(crate) fn heading(&self) -> &str {
        &self.heading
    }
}

impl UserData for LuaSectionHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name.clone()));
        fields.add_field_method_get("has_prose", |_, this| Ok(this.has_prose));
    }
}

/// One fanout arm result exposed to Lua as a structured object.
///
/// Authors read `.text`, `.ok`, `.item`, and `.exhausted`. `__tostring` returns
/// `.text` so `tostring` and a tostring-coercing `table.concat` keep working.
/// `.item` carries the arm's member value back as a Lua value via the same
/// serde bridge that seeds `var`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LuaFanoutResult {
    text: String,
    ok: bool,
    item: Json,
    exhausted: bool,
}

impl LuaFanoutResult {
    /// Builds a successful arm result.
    #[must_use]
    pub(crate) fn success(item: impl Into<Json>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ok: true,
            item: item.into(),
            exhausted: false,
        }
    }

    /// Builds a soft-degraded arm result after tool-loop exhaustion.
    #[must_use]
    pub(crate) fn exhausted_stub(item: impl Into<Json>, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ok: false,
            item: item.into(),
            exhausted: true,
        }
    }
}

impl UserData for LuaFanoutResult {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("text", |_, this| Ok(this.text.clone()));
        fields.add_field_method_get("ok", |_, this| Ok(this.ok));
        fields.add_field_method_get("item", |lua, this| lua.to_value(&this.item));
        fields.add_field_method_get("exhausted", |_, this| Ok(this.exhausted));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.text.clone()));
    }
}

/// Outcome of a Lua block that may invoke `jump`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LuaBlockResult {
    /// Normal completion with an optional scalar return.
    Returned(Option<String>),
    /// `jump` transferred control to this heading (`## Name`).
    Jump(String),
}

/// Resolves an `execute` / `jump` target from a heading string or Section object.
///
/// # Errors
/// Returns a Lua error when the value is neither a string nor a Section handle.
pub(crate) fn resolve_section_target(value: Value) -> mlua::Result<String> {
    match value {
        Value::String(s) => Ok(s.to_str()?.to_owned()),
        Value::UserData(ud) => {
            let handle = ud.borrow::<LuaSectionHandle>()?;
            Ok(handle.heading().to_owned())
        }
        other => Err(mlua::Error::external(format!(
            "section target must be a string or Section object, got {}",
            other.type_name()
        ))),
    }
}

/// Immutable prompt-level tool bindings produced by live H1 execution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolBindings {
    pub(crate) bindings: Vec<ToolBinding>,
    pub(crate) always: Vec<String>,
}

impl ToolBindings {
    #[cfg(test)]
    pub(crate) fn for_test(bindings: Vec<ToolBinding>, always: Vec<String>) -> Self {
        Self { bindings, always }
    }

    /// Returns bindings in declaration order.
    #[must_use]
    pub(crate) fn bindings(&self) -> &[ToolBinding] {
        &self.bindings
    }

    /// Returns prompt-wide aliases in declaration order.
    #[must_use]
    pub(crate) fn always(&self) -> &[String] {
        &self.always
    }

    pub(crate) fn binding(&self, alias: &str) -> Option<&ToolBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }
}
