//! Inspectable Lua userdata returned by `models.bind` / `models.default`.
//!
//! Presentation only: the userdata exposes a frozen [`ModelBinding`]'s fields
//! to Lua. Invocation is namespace-only (A9): the handle carries no methods,
//! and `models.infer(handle?, prompt)` takes it as an optional leading
//! argument.

use std::sync::Arc;

use mlua::{Lua, UserData, UserDataFields};

use promptforge_model_client::model::ModelBinding;

/// Host hook that runs `models.infer` from Lua via the executor's shared
/// context.
///
/// Takes only the prompt: the hook resolves the section's current model
/// binding itself, because the executor side knows the section name needed
/// for a typed model-required failure and, on the live H1 path, the
/// bindings are still being recorded into the run's producer.
/// Installed as Lua app data; absent app data means `models.infer` is
/// unavailable in that context.
pub(crate) type ModelsInferHook = Arc<dyn Fn(&Lua, &str) -> mlua::Result<String> + Send + Sync>;

/// Inspectable Lua userdata returned by `models.bind` / `models.default`.
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

    /// Returns the frozen binding carried by this handle.
    #[must_use]
    pub(crate) fn binding(&self) -> &ModelBinding {
        &self.binding
    }

    /// Returns the prompt-local alias.
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        self.binding.alias()
    }

    /// Returns the caller-facing catalog model id.
    #[must_use]
    pub(crate) fn model_id(&self) -> &str {
        self.binding.id().name()
    }

    /// Returns the capability description supplied to `models.bind`.
    #[must_use]
    pub(crate) fn description(&self) -> &str {
        self.binding.description()
    }

    /// Returns the catalog context window size in tokens.
    ///
    /// The binding stores a [`NonZeroU32`](std::num::NonZeroU32); the raw `u32`
    /// is exposed only here, at the Lua presentation boundary.
    #[must_use]
    pub(crate) fn context(&self) -> u32 {
        self.binding.context().get()
    }

    /// Returns the frozen thinking switch, when the bind declared one.
    #[must_use]
    pub(crate) fn thinking(&self) -> Option<bool> {
        self.binding.invocation().thinking
    }

    /// Returns the frozen sampling temperature, when the bind declared one.
    ///
    /// The binding stores a validated
    /// [`Temperature`](promptforge_model_client::model::Temperature); the
    /// raw `f64` is exposed only here, at the Lua presentation boundary.
    #[must_use]
    pub(crate) fn temperature(&self) -> Option<f64> {
        self.binding
            .invocation()
            .temperature
            .map(promptforge_model_client::model::Temperature::get)
    }

    /// Returns the frozen max generation tokens, when the bind declared one.
    ///
    /// The binding stores a [`NonZeroU32`](std::num::NonZeroU32); the raw `u32`
    /// is exposed only here, at the Lua presentation boundary.
    #[must_use]
    pub(crate) fn max_tokens(&self) -> Option<u32> {
        self.binding
            .invocation()
            .max_tokens
            .map(std::num::NonZeroU32::get)
    }
}

impl UserData for LuaModelHandle {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, this| Ok(this.name().to_owned()));
        fields.add_field_method_get("model_id", |_, this| Ok(this.model_id().to_owned()));
        fields.add_field_method_get("description", |_, this| Ok(this.description().to_owned()));
        fields.add_field_method_get("context", |_, this| Ok(this.context()));
        fields.add_field_method_get("thinking", |_, this| Ok(this.thinking()));
        fields.add_field_method_get("temperature", |_, this| Ok(this.temperature()));
        fields.add_field_method_get("max_tokens", |_, this| Ok(this.max_tokens()));
    }
}
