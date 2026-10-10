//! Inspectable Lua userdata returned by `models.use` / `models.default` /
//! `models.get`.
//!
//! The userdata exposes a frozen [`ModelBinding`]'s fields to Lua, and it
//! carries `infer` and `loop`, run as `h:infer(prompt)` and
//! `h:loop(messages, compactor?)` on the handle's binding. Those two are
//! the shim's Lua functions, read through field getters, because each may
//! suspend and a Rust method cannot.

use mlua::{UserData, UserDataFields, Value};

use promptforge_model_client::model::ModelBinding;

/// Inspectable Lua userdata returned by `models.use` / `models.default` /
/// `models.get`.
#[derive(Debug, Clone)]
pub(crate) struct LuaModelHandle {
    binding: ModelBinding,
}

impl LuaModelHandle {
    /// Builds a handle from a frozen [`ModelBinding`].
    #[must_use]
    pub(crate) fn from_binding(binding: &ModelBinding) -> Self {
        Self {
            binding: binding.clone(),
        }
    }

    /// Returns the frozen binding this handle holds.
    #[must_use]
    pub(crate) fn binding(&self) -> &ModelBinding {
        &self.binding
    }

    /// Returns the prompt-local alias.
    #[must_use]
    fn name(&self) -> &str {
        self.binding.alias()
    }

    /// Returns the role label the binding filled (the alias, under the
    /// frontmatter's role vocabulary).
    #[must_use]
    fn label(&self) -> &str {
        self.binding.alias()
    }

    /// Returns the bound role's full keyword set.
    #[must_use]
    fn capabilities(&self) -> &[String] {
        self.binding.capabilities()
    }

    /// Returns the caller-facing catalog model id.
    #[must_use]
    fn model_id(&self) -> &str {
        self.binding.id().name()
    }

    /// Returns the capability description of the bound role.
    #[must_use]
    fn description(&self) -> &str {
        self.binding.description()
    }

    /// Returns the catalog context window size in tokens.
    ///
    /// The binding stores a [`NonZeroU32`](std::num::NonZeroU32); the raw `u32`
    /// is exposed only here, at the Lua presentation boundary.
    #[must_use]
    fn context(&self) -> u32 {
        self.binding.context().get()
    }

    /// Returns the frozen thinking switch, when the role declared one.
    #[must_use]
    fn thinking(&self) -> Option<bool> {
        self.binding.invocation().thinking
    }

    /// Returns the frozen sampling temperature, when the role declared one.
    ///
    /// The binding stores a validated
    /// [`Temperature`](promptforge_model_client::model::Temperature); the
    /// raw `f64` is exposed only here, at the Lua presentation boundary.
    #[must_use]
    fn temperature(&self) -> Option<f64> {
        self.binding
            .invocation()
            .temperature
            .map(promptforge_model_client::model::Temperature::get)
    }

    /// Returns the frozen max generation tokens, when the role declared one.
    ///
    /// The binding stores a [`NonZeroU32`](std::num::NonZeroU32); the raw `u32`
    /// is exposed only here, at the Lua presentation boundary.
    #[must_use]
    fn max_tokens(&self) -> Option<u32> {
        self.binding
            .invocation()
            .max_tokens
            .map(std::num::NonZeroU32::get)
    }
}

impl UserData for LuaModelHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name().to_owned()));
        fields.add_field_method_get("label", |_, this| Ok(this.label().to_owned()));
        fields.add_field_method_get("capabilities", |lua, this| {
            lua.create_sequence_from(this.capabilities().to_vec())
        });
        fields.add_field_method_get("model_id", |_, this| Ok(this.model_id().to_owned()));
        fields.add_field_method_get("description", |_, this| Ok(this.description().to_owned()));
        fields.add_field_method_get("context", |_, this| Ok(this.context()));
        fields.add_field_method_get("thinking", |_, this| Ok(this.thinking()));
        fields.add_field_method_get("temperature", |_, this| Ok(this.temperature()));
        fields.add_field_method_get("max_tokens", |_, this| Ok(this.max_tokens()));
        fields.add_field_function_get("infer", |lua, _| crate::coro::handle_method(lua, "infer"));
        fields.add_field_function_get("loop", |lua, _| crate::coro::handle_method(lua, "loop"));
    }
}

/// Whether `value` is a model handle.
pub(crate) fn is_handle(value: &Value) -> bool {
    matches!(value, Value::UserData(userdata) if userdata.is::<LuaModelHandle>())
}
