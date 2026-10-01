//! Section VM state reads: `var`, bare globals, `argv`, the tool call
//! counts, the shared tool and model sets, and the section's effective
//! tool and model bindings.

use promptforge_model_client::client::ToolSchema;

use super::SectionVm;
#[cfg(any(test, feature = "test-support"))]
use crate::{Arc, ModelSet};
use crate::{
    Error, Json, Lua, LuaSerdeExt, ModelBinding, ModelRuntime, ModelView, Mutex, Result,
    ToolBinding, ToolCallCounts, ToolRuntime, ToolSet, Value,
    install_tool_call_counts as install_tool_call_counts_impl, var_to_json,
};

impl SectionVm {
    /// Returns the current `var` table as JSON, read from the hidden data
    /// table behind the guarded proxy (not the proxy, which stays empty).
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if Engine values have not been injected or `var`
    /// cannot be represented as JSON.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_lua::SectionVm;
    /// use promptforge_types::emitter::{DebugMode, Emitter, EventSink};
    /// use promptforge_types::untrusted::GuardNonce;
    ///
    /// let nonce = GuardNonce::from_seed(1);
    /// let emitter = Emitter::root(EventSink::default(), "example-run", DebugMode::Off);
    /// let vfs = promptforge_vfs::VfsRef::default();
    /// let access = std::sync::Arc::new(
    ///     vfs.acquire(promptforge_vfs::Origin::new("vm example"))?,
    /// );
    /// let mut vm = SectionVm::new(&nonce, &emitter, "Example")?;
    /// vm.inject_host("", &serde_json::json!({}), &access)?;
    /// assert_eq!(vm.var()?, serde_json::json!({}));
    /// vm.teardown(&emitter, "Example");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn var(&self) -> Result<Json> {
        if !self.host_injected {
            return Err(Error::Lua(
                "section VM host values have not been injected".to_owned(),
            ));
        }
        var_to_json(&self.lua)
    }

    /// Reads a bare global for prose substitution: `None` when the global is
    /// unset, its JSON form when set.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when the global is a function, userdata, or
    /// thread (bare globals in prose must be data), or when its value cannot
    /// be represented as JSON.
    pub fn global_json(&self, name: &str) -> Result<Option<Json>> {
        let value: Value = self.lua.globals().get(name).map_err(Error::lua)?;
        match value {
            Value::Nil => Ok(None),
            Value::Function(_) | Value::UserData(_) | Value::Thread(_) => Err(Error::Lua(format!(
                "global `{name}` is a {}; bare globals in prose must be JSON data",
                value.type_name()
            ))),
            other => Ok(Some(self.lua.from_value(other).map_err(Error::lua)?)),
        }
    }

    /// Reads the `argv` global back as JSON at the H1 freeze: `None` when
    /// nil, its JSON form otherwise. Call this on the H1 VM only - a frozen
    /// section's `argv` sits behind the guard proxy, which is not the
    /// read-back path.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when H1 left `argv` as a function, userdata,
    /// or thread, or when its value cannot be represented as JSON.
    pub fn argv_json(&self) -> Result<Option<Json>> {
        let value: Value = self.lua.globals().get("argv").map_err(Error::lua)?;
        match value {
            Value::Nil => Ok(None),
            Value::Function(_) | Value::UserData(_) | Value::Thread(_) => Err(Error::Lua(format!(
                "argv must be JSON data, got {}",
                value.type_name()
            ))),
            other => Ok(Some(self.lua.from_value(other).map_err(Error::lua)?)),
        }
    }

    /// Sets a global in the VM to the Lua form of a JSON value, overwriting
    /// any existing value.
    ///
    /// Used by fanout to inject `item` after Engine injection; the conversion
    /// is the same `LuaSerdeExt` bridge that seeds `var`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the value cannot convert or the global
    /// cannot be set.
    pub fn set_global_json(&self, name: &str, value: &Json) -> Result<()> {
        let value = self.lua.to_value(value).map_err(Error::lua)?;
        self.lua.globals().raw_set(name, value).map_err(Error::lua)
    }

    /// Installs `tools.calls` as a read-only Lua table backed by a fresh
    /// [`ToolCallCounts`]. Each seeded alias reads its live count; indexing
    /// an unseeded key is a hard error that names the bad key and lists the
    /// seeded set. When the key names a bound tool slot but was never
    /// seeded - neither scoped into the section nor dispatched by a script
    /// `tools.call` - the diagnostic says so.
    ///
    /// The installation itself sits in the `tools` module; this method only
    /// supplies the VM's own state.
    ///
    /// Returns the `ToolCallCounts` handle so the executor's tool loop can
    /// increment it.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when installing the `tools.calls` index fails
    /// or the shared tool set's mutex is poisoned.
    pub fn install_tool_call_counts(&self, bindings: &[ToolBinding]) -> Result<ToolCallCounts> {
        let declared = self
            .bound_tools
            .lock()
            .map_err(|_| Error::Lua("tool set mutex was poisoned".to_owned()))?
            .clone();
        install_tool_call_counts_impl(&self.lua, &declared, bindings)
    }

    /// Returns a snapshot of the shared tool set and the live section
    /// addition runtime.
    ///
    /// A test helper for `promptforge-engine`'s executor tests, so it exists
    /// only under `test-support`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the shared tool set's mutex is poisoned.
    #[cfg(any(test, feature = "test-support"))]
    pub fn tool_bag_handles(&self) -> Result<(ToolSet, Arc<Mutex<ToolRuntime>>)> {
        let tools = self
            .bound_tools
            .lock()
            .map_err(|_| Error::Lua("tool set mutex was poisoned".to_owned()))?
            .clone();
        Ok((tools, Arc::clone(&self.tool_runtime)))
    }

    /// Returns a snapshot of the shared model set and the live section
    /// selection runtime.
    ///
    /// Test-only: production reads the run's shared set through the model
    /// view; tests snapshot straight from the VM, so it exists only under
    /// `test-support`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the shared model set's mutex is poisoned.
    #[cfg(any(test, feature = "test-support"))]
    pub fn model_bag_handles(&self) -> Result<(ModelSet, Arc<Mutex<ModelRuntime>>)> {
        let models = self
            .bound_models
            .lock()
            .map_err(|_| Error::Lua("model set mutex was poisoned".to_owned()))?
            .clone();
        Ok((models, Arc::clone(&self.model_runtime)))
    }

    /// Borrows the inner Lua state, so the shim installs and the
    /// scheduler's scoped H1 steps can drive coroutines on the VM.
    #[must_use]
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Returns the schemas of every registered local tool.
    /// # Errors
    /// Returns [`Error::Lua`] if the local-tools registry was poisoned.
    pub fn local_tool_schemas(&self) -> Result<Vec<ToolSchema>> {
        self.local_tools.schemas()
    }

    /// Returns whether `alias` names a registered local tool.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the local-tools registry was poisoned.
    pub fn has_local_tool(&self, alias: &str) -> Result<bool> {
        self.local_tools.contains(alias)
    }
}

/// Reads the section's effective tool bindings without mutating the tool
/// runtime: prompt-wide `always` aliases followed by H2 `tools.add`
/// additions, each resolved against the frozen bindings with any author
/// description override applied.
///
/// Rebuilt at each model operation so `tools.add` and `tools.add_local`
/// calls between blocks reach the next model turn.
///
/// # Errors
/// Returns [`Error::Lua`] if the tool runtime's mutex is poisoned or an added
/// alias has no frozen binding.
pub fn current_tool_bindings(
    bindings: &ToolSet,
    runtime: &Mutex<ToolRuntime>,
) -> Result<Vec<ToolBinding>> {
    let runtime = runtime
        .lock()
        .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
    bindings
        .always()
        .iter()
        .chain(runtime.added.iter())
        .map(|alias| binding_for_scope(bindings, &runtime, alias))
        .collect()
}

/// Reads the section's effective model binding through the run's model view
/// without mutating the model runtime: the H2 `models.use` selection with
/// its options applied, else the prompt-wide `models.default` baseline.
///
/// # Errors
/// Returns [`Error::Lua`] if the model runtime's mutex is poisoned or the
/// selected alias has no frozen binding.
pub fn resolve_model_binding(
    bindings: &dyn ModelView,
    runtime: &Mutex<ModelRuntime>,
) -> Result<Option<ModelBinding>> {
    let selection = {
        let runtime = runtime
            .lock()
            .map_err(|_| Error::Lua("model declaration runtime was poisoned".to_owned()))?;
        runtime
            .selection()
            .map(|(alias, options)| (alias.to_owned(), options))
    };
    let frozen = |alias: &str| -> Result<ModelBinding> {
        bindings
            .binding(alias)?
            .ok_or_else(|| Error::Lua(format!("model alias {alias:?} has no frozen binding")))
    };
    match selection {
        Some((alias, options)) => Ok(Some(options.apply(frozen(&alias)?))),
        None => match bindings.default()? {
            Some(alias) => Ok(Some(frozen(&alias)?)),
            None => Ok(None),
        },
    }
}

/// Clones a frozen binding and applies any author model-description override.
pub(crate) fn binding_for_scope(
    bindings: &ToolSet,
    runtime: &ToolRuntime,
    alias: &str,
) -> Result<ToolBinding> {
    let mut binding = bindings
        .binding(alias)
        .cloned()
        .ok_or_else(|| Error::Lua(format!("tool alias {alias:?} has no frozen binding")))?;
    if let Some(description) = runtime.description_overrides.get(alias) {
        binding.model_description = Some(description.clone());
    }
    Ok(binding)
}
