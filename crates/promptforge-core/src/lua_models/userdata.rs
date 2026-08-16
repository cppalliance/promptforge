//! Inspectable Lua userdata returned by `models.need` / `models.only`.
//!
//! Presentation only: the userdata exposes a frozen [`ModelBinding`]'s fields to
//! Lua and dispatches `model:infer` through the executor's installed hook.

use std::sync::Arc;

use mlua::{Lua, UserData, UserDataFields, UserDataMethods, Value};

use crate::dialects::ToolDialectId;
use crate::model::ModelBinding;

/// Host hook that runs `model:infer` from Lua via the executor's shared context.
///
/// Installed as Lua app data for the duration of a section phase that may call
/// infer. Absent app data means infer is unavailable in that context.
pub(crate) type ModelInferHook =
    Arc<dyn Fn(&Lua, &ModelBinding, &str) -> mlua::Result<String> + Send + Sync>;

/// Inspectable Lua userdata returned by `models.need` / `models.only`.
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

    /// Returns the capability description supplied to `models.need`.
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

    /// Returns the frozen thinking switch, when the need declared one.
    #[must_use]
    pub(crate) fn thinking(&self) -> Option<bool> {
        self.binding.invocation().thinking
    }

    /// Returns the frozen sampling temperature, when the need declared one.
    ///
    /// The binding stores a validated [`crate::model::Temperature`]; the raw
    /// `f64` is exposed only here, at the Lua presentation boundary.
    #[must_use]
    pub(crate) fn temperature(&self) -> Option<f64> {
        self.binding
            .invocation()
            .temperature
            .map(crate::model::Temperature::get)
    }

    /// Returns the frozen max generation tokens, when the need declared one.
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

    /// Returns the tool-calling dialect id.
    ///
    /// Returns the closed [`ToolDialectId`] identity; the `String` allocation
    /// happens only in the Lua userdata field callback, at the boundary that
    /// actually needs a Lua string.
    #[must_use]
    pub(crate) fn dialect(&self) -> ToolDialectId {
        self.binding.tool_dialect()
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
        fields.add_field_method_get("dialect", |_, this| Ok(this.dialect().to_string()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "infer",
            |lua, this, (prompt, opts): (String, Option<Value>)| {
                // Per-call options are not supported. Reject them explicitly so
                // an author-supplied table can never be silently discarded.
                reject_infer_options(opts.as_ref())?;
                let hook = lua
                    .app_data_ref::<ModelInferHook>()
                    .ok_or_else(|| {
                        mlua::Error::external(
                            "model:infer is not available outside section execution",
                        )
                    })?
                    .clone();
                hook(lua, this.binding(), &prompt)
            },
        );
    }
}

/// Rejects any per-call `model:infer` options argument.
///
/// `model:infer` takes only a prompt string. A second argument (a table of
/// options, or anything non-nil) has no supported effect, so it is rejected
/// rather than silently dropped. An absent or `nil` second argument is allowed.
pub(crate) fn reject_infer_options(opts: Option<&Value>) -> mlua::Result<()> {
    match opts {
        None | Some(Value::Nil) => Ok(()),
        Some(_) => Err(mlua::Error::external(
            "model:infer(prompt) does not accept a second argument; \
             per-call inference options are not supported",
        )),
    }
}
