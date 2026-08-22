use super::{
    Arc, Conflict, Error, Lua, LuaToolHandle, ModelBindingState, ModelBindings, ModelResolver,
    MultiValue, Mutex, Result, ToolBinding, ToolCatalog, ToolResolver, ToolSet,
    install_live_models,
};

/// Records the first concrete callback error, preserving its typed cause.
fn record_callback_error(errors: &Mutex<Option<Error>>, error: Error) -> mlua::Result<()> {
    let mut slot = errors
        .lock()
        .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
    if slot.is_none() {
        *slot = Some(error);
    }
    Ok(())
}

/// Run-scoped accumulator populated by live H1 capability calls.
///
/// The producer is installed into one H1 VM. Every executed `tools.bind`,
/// `models.bind`, and `models.default` call resolves immediately, while skipped
/// Lua branches produce no binding. The tool half writes through the run's
/// shared [`ToolSet`] handle - the same allocation the run context reads
/// through its `ToolView` - so the walk needs no bindings handoff.
#[derive(Debug, Clone)]
pub(crate) struct LiveBindingProducer {
    tools: Arc<Mutex<ToolSet>>,
    tool_error: Arc<Mutex<Option<Error>>>,
    models: Arc<Mutex<ModelBindingState>>,
}

impl LiveBindingProducer {
    /// Builds a producer whose tool bindings land in the run's shared set.
    pub(crate) fn new(tools: Arc<Mutex<ToolSet>>) -> Self {
        Self {
            tools,
            tool_error: Arc::new(Mutex::new(None)),
            models: Arc::new(Mutex::new(ModelBindingState::default())),
        }
    }

    /// Installs live tool and model tables into `lua` for the lifetime of
    /// `scope`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] when either table cannot be installed.
    pub(crate) fn install<'scope, 'env: 'scope>(
        &self,
        lua: &'env Lua,
        scope: &'scope mlua::Scope<'scope, 'env>,
        tool_resolver: &'env dyn ToolResolver,
        catalog: &'env ToolCatalog,
        model_resolver: &'env dyn ModelResolver,
    ) -> Result<()> {
        install_live_tools(
            lua,
            scope,
            tool_resolver,
            catalog,
            &self.tools,
            &self.tool_error,
        )?;
        install_live_models(lua, scope, model_resolver, &self.models)
    }

    /// Returns the first concrete resolver error captured by a Lua callback.
    ///
    /// This lets the H1 executor preserve typed resolution errors instead of
    /// replacing them with mlua's callback wrapper.
    pub(crate) fn take_callback_error(&self) -> Result<Option<Error>> {
        let tool_error = self
            .tool_error
            .lock()
            .map_err(|_| Error::Lua("tool binding recorder was poisoned".to_owned()))?
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
    pub(crate) fn bindings(&self) -> Result<(ToolSet, ModelBindings)> {
        let tools = self
            .tools
            .lock()
            .map_err(|_| Error::Lua("tool binding recorder was poisoned".to_owned()))?;
        let models = self
            .models
            .lock()
            .map_err(|_| Error::Lua("model binding recorder was poisoned".to_owned()))?;
        Ok((
            tools.clone(),
            ModelBindings::from_parts(models.bindings.clone(), models.default.clone()),
        ))
    }

    /// Snapshots the model bindings resolved by the live H1 execution.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the recorder mutex is poisoned.
    pub(crate) fn models(&self) -> Result<ModelBindings> {
        let models = self
            .models
            .lock()
            .map_err(|_| Error::Lua("model binding recorder was poisoned".to_owned()))?;
        Ok(ModelBindings::from_parts(
            models.bindings.clone(),
            models.default.clone(),
        ))
    }
}

/// Installs live H1 tool resolution into an existing Lua VM.
///
/// `tools.bind` consults `resolver` at the point Lua executes the call, verifies
/// the selected identity against the run's tool catalog, attaches the
/// resolved implementation to the recorded binding, and returns an inspectable
/// Tool object populated from it. Each successful bind also runs the
/// near-duplicate scan of the new identity against every existing binding's
/// identity, recording each clash symmetrically on both bindings; binding
/// records, never fails - a clash errors only when both halves later enter
/// one model-visible scope.
///
/// # Errors
/// Returns [`Error::Lua`] when the Lua table cannot be installed.
#[expect(
    clippy::too_many_lines,
    reason = "one scoped table keeps its callbacks and shared recorder together"
)]
pub(crate) fn install_live_tools<'scope, 'env: 'scope>(
    lua: &'env Lua,
    scope: &'scope mlua::Scope<'scope, 'env>,
    resolver: &'env dyn ToolResolver,
    catalog: &'env ToolCatalog,
    set: &Arc<Mutex<ToolSet>>,
    errors: &Arc<Mutex<Option<Error>>>,
) -> Result<()> {
    let tools = lua.create_table().map_err(Error::lua)?;

    let bind_state = Arc::clone(set);
    let bind_errors = Arc::clone(errors);
    let bind = scope
        .create_function(
            move |_,
                  (alias, description, model_description): (String, String, Option<String>)|
                  -> mlua::Result<LuaToolHandle> {
                validate_alias(&alias).map_err(mlua::Error::external)?;
                {
                    let bindings = bind_state
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
                        drop(bindings);
                        record_callback_error(&bind_errors, error)?;
                        return Err(mlua::Error::external("duplicate tool alias"));
                    }
                }
                let id = match resolver.resolve(&description) {
                    Ok(id) => id,
                    Err(error) => {
                        record_callback_error(&bind_errors, error)?;
                        return Err(mlua::Error::external("tool capability resolution failed"));
                    }
                };
                let Some(tool) = catalog.get(&id) else {
                    let error = Error::PickedToolNotLive {
                        alias: alias.clone(),
                        id,
                    };
                    record_callback_error(&bind_errors, error)?;
                    return Err(mlua::Error::external("picked tool is not live"));
                };
                let handle = LuaToolHandle::from_live_binding(&alias, &description, tool.as_ref());
                // The identity guard and the scan's id list read under one
                // short lock; the picker query itself runs unlocked, since
                // the resolver is a re-entrant capability.
                let mut ids = {
                    let bindings = bind_state
                        .lock()
                        .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                    if let Some(first) = bindings
                        .bindings
                        .iter()
                        .find(|binding| binding.id == id)
                        .map(|binding| binding.alias.clone())
                    {
                        let error = Error::ToolIdSelectedTwice {
                            id: id.clone(),
                            first_alias: first,
                            second_alias: alias.clone(),
                        };
                        drop(bindings);
                        record_callback_error(&bind_errors, error)?;
                        return Err(mlua::Error::external(
                            "tool identity was selected more than once",
                        ));
                    }
                    bindings
                        .bindings
                        .iter()
                        .map(|binding| binding.id.clone())
                        .collect::<Vec<_>>()
                };
                ids.push(id.clone());
                // A lone binding has no pairs; skip the query.
                let pairs = if ids.len() > 1 {
                    match resolver.near_duplicates(&ids) {
                        Ok(pairs) => pairs,
                        Err(error) => {
                            record_callback_error(&bind_errors, error)?;
                            return Err(mlua::Error::external("tool conflict analysis failed"));
                        }
                    }
                } else {
                    Vec::new()
                };
                let mut bindings = bind_state
                    .lock()
                    .map_err(|_| mlua::Error::external("tool binding recorder was poisoned"))?;
                // Record each clash symmetrically. The picker reports pairs
                // among exactly the ids supplied, so a pair not touching the
                // new binding was already recorded when its later half bound.
                let mut conflicts = Vec::new();
                for (first, second, similarity) in pairs {
                    let other = if second == id {
                        first
                    } else if first == id {
                        second
                    } else {
                        continue;
                    };
                    let Some(existing) = bindings
                        .bindings
                        .iter_mut()
                        .find(|binding| binding.id == other)
                    else {
                        continue;
                    };
                    let similarity = f64::from(similarity);
                    conflicts.push(Conflict {
                        alias: existing.alias.clone(),
                        similarity,
                    });
                    existing.conflicts.push(Conflict {
                        alias: alias.clone(),
                        similarity,
                    });
                }
                bindings.bindings.push(ToolBinding {
                    alias: alias.clone(),
                    description: description.clone(),
                    id,
                    model_description,
                    tool,
                    conflicts,
                });
                Ok(handle)
            },
        )
        .map_err(Error::lua)?;
    tools.set("bind", bind).map_err(Error::lua)?;

    let prompt_wide = Arc::clone(set);
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
                        "tools.always alias {alias:?} was not declared by tools.bind"
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

/// Validates a prompt-local tool alias against the supported wire grammar.
///
/// # Errors
/// Returns [`Error::Lua`] when `alias` is empty, exceeds 64 bytes, starts with
/// a non-letter, or contains a character other than a letter, digit, `_`, or
/// `-` after its first byte.
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
