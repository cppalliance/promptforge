use super::{
    Arc, Error, Lua, LuaToolHandle, ModelBindingState, ModelBindings, ModelResolver, MultiValue,
    Mutex, Result, ToolBinding, ToolBindings, ToolRegistry, ToolResolver, install_live_models,
};

#[derive(Debug, Default)]
pub(crate) struct BindingState {
    bindings: Vec<ToolBinding>,
    always: Vec<String>,
    callback_error: Option<Error>,
}

impl BindingState {
    /// Records the first concrete callback error, preserving its typed cause.
    fn record_error(&mut self, error: Error) {
        if self.callback_error.is_none() {
            self.callback_error = Some(error);
        }
    }
}

/// Run-scoped accumulator populated by live H1 capability calls.
///
/// The producer is installed into one H1 VM. Every executed `tools.need`,
/// `models.need`, and `models.default` call resolves immediately, while skipped
/// Lua branches produce no binding.
#[derive(Debug, Clone, Default)]
pub(crate) struct LiveBindingProducer {
    tools: Arc<Mutex<BindingState>>,
    models: Arc<Mutex<ModelBindingState>>,
}

impl LiveBindingProducer {
    /// Installs live tool and model tables into `lua` for the lifetime of
    /// `scope`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when either table cannot be installed.
    pub(crate) fn install<'scope, 'env: 'scope, 'tools: 'env>(
        &self,
        lua: &'env Lua,
        scope: &'scope mlua::Scope<'scope, 'env>,
        tool_resolver: &'env dyn ToolResolver,
        registry: &'env ToolRegistry<'tools>,
        model_resolver: &'env dyn ModelResolver,
    ) -> Result<()> {
        install_live_tools(lua, scope, tool_resolver, registry, &self.tools)?;
        install_live_models(lua, scope, model_resolver, &self.models)
    }

    /// Returns the first concrete resolver error captured by a Lua callback.
    ///
    /// This lets the H1 executor preserve typed resolution errors instead of
    /// replacing them with mlua's callback wrapper.
    pub(crate) fn take_callback_error(&self) -> Result<Option<Error>> {
        let tool_error = self
            .tools
            .lock()
            .map_err(|_| Error::Lua("tool binding recorder was poisoned".to_owned()))?
            .callback_error
            .take();
        let model_error = self
            .models
            .lock()
            .map_err(|_| Error::Lua("model binding recorder was poisoned".to_owned()))?
            .callback_error
            .take();
        Ok(tool_error.or(model_error))
    }

    /// Snapshots all bindings resolved by the live H1 execution so far.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if either recorder mutex is poisoned.
    pub(crate) fn bindings(&self) -> Result<(ToolBindings, ModelBindings)> {
        let tools = self
            .tools
            .lock()
            .map_err(|_| Error::Lua("tool binding recorder was poisoned".to_owned()))?;
        let models = self
            .models
            .lock()
            .map_err(|_| Error::Lua("model binding recorder was poisoned".to_owned()))?;
        Ok((
            ToolBindings {
                bindings: tools.bindings.clone(),
                always: tools.always.clone(),
            },
            ModelBindings::from_parts(models.bindings.clone(), models.default.clone()),
        ))
    }
}

/// Installs live H1 tool resolution into an existing Lua VM.
///
/// `tools.need` consults `resolver` at the point Lua executes the call, verifies
/// the selected identity against `registry`, records the frozen binding, and
/// returns an inspectable Tool object populated from the live registry entry.
///
/// # Errors
/// Returns [`Error::Lua`] when the Lua table cannot be installed.
#[expect(
    clippy::too_many_lines,
    reason = "one scoped table keeps its callbacks and shared recorder together"
)]
pub(crate) fn install_live_tools<'scope, 'env: 'scope, 'tools: 'env>(
    lua: &'env Lua,
    scope: &'scope mlua::Scope<'scope, 'env>,
    resolver: &'env dyn ToolResolver,
    registry: &'env ToolRegistry<'tools>,
    state: &Arc<Mutex<BindingState>>,
) -> Result<()> {
    let tools = lua.create_table().map_err(Error::lua)?;

    let needs = Arc::clone(state);
    let need = scope
        .create_function(
            move |_,
                  (alias, description, model_description): (String, String, Option<String>)|
                  -> mlua::Result<LuaToolHandle> {
                validate_alias(&alias).map_err(mlua::Error::external)?;
                {
                    let mut bindings = needs
                        .lock()
                        .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                    if bindings
                        .bindings
                        .iter()
                        .any(|binding| binding.alias == alias)
                    {
                        let error = Error::DuplicateAlias {
                            alias: alias.clone(),
                        };
                        bindings.record_error(error);
                        return Err(mlua::Error::external("duplicate tool alias"));
                    }
                }
                let id = match resolver.resolve(&description) {
                    Ok(id) => id,
                    Err(error) => {
                        let mut bindings = needs.lock().map_err(|_| {
                            mlua::Error::external("tool binding recorder was poisoned")
                        })?;
                        bindings.record_error(error);
                        return Err(mlua::Error::external("tool capability resolution failed"));
                    }
                };
                let Some(tool) = registry.get(&id) else {
                    let error = Error::PickedToolNotLive {
                        alias: alias.clone(),
                        id,
                    };
                    let mut bindings = needs
                        .lock()
                        .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                    bindings.record_error(error);
                    return Err(mlua::Error::external("picked tool is not live"));
                };
                let handle = LuaToolHandle::from_live_binding(&alias, &description, tool);
                let mut bindings = needs
                    .lock()
                    .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                if let Some(first) = bindings
                    .bindings
                    .iter()
                    .find(|binding| binding.id == id)
                    .map(|binding| binding.alias.clone())
                {
                    let error = Error::ToolIdSelectedTwice {
                        id,
                        first_alias: first,
                        second_alias: alias,
                    };
                    bindings.record_error(error);
                    return Err(mlua::Error::external(
                        "tool identity was selected more than once",
                    ));
                }
                bindings.bindings.push(ToolBinding {
                    alias: alias.clone(),
                    description: description.clone(),
                    id,
                    model_description,
                });
                Ok(handle)
            },
        )
        .map_err(Error::lua)?;
    tools.set("need", need).map_err(Error::lua)?;

    let prompt_wide = Arc::clone(state);
    let always = scope
        .create_function(
            move |_, (alias, model_description): (String, Option<String>)| -> mlua::Result<()> {
                validate_alias(&alias).map_err(mlua::Error::external)?;
                let mut bindings = prompt_wide
                    .lock()
                    .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                if bindings.always.iter().any(|existing| existing == &alias) {
                    return Err(mlua::Error::external(format!(
                        "tools.always alias {alias:?} was recorded more than once"
                    )));
                }
                let Some(binding) = bindings
                    .bindings
                    .iter_mut()
                    .find(|binding| binding.alias == alias)
                else {
                    return Err(mlua::Error::external(format!(
                        "tools.always alias {alias:?} was not declared by tools.need"
                    )));
                };
                if let Some(model_description) = model_description {
                    binding.model_description = Some(model_description);
                }
                bindings.always.push(alias);
                Ok(())
            },
        )
        .map_err(Error::lua)?;
    tools.set("always", always).map_err(Error::lua)?;

    let add = scope
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "tools.add is only available during H2 recording",
            ))
        })
        .map_err(Error::lua)?;
    tools.set("add", add).map_err(Error::lua)?;
    lua.globals().raw_set("tools", tools).map_err(Error::lua)
}

pub(crate) fn validate_alias(alias: &str) -> Result<()> {
    let bytes = alias.as_bytes();
    let valid = (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_alphabetic()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(Error::Lua(format!(
            "invalid tool alias {alias:?}: expected [A-Za-z][A-Za-z0-9_-]{{0,63}}"
        )))
    }
}
