//! Lua `models.bind` / `models.use` host tables for live H1 and H2.
//!
//! Kept beside [`crate::lua`] so the tool tables stay readable while model
//! declaration recording mirrors their phase rules.

use std::sync::Arc;
use std::sync::Mutex;

use mlua::{Lua, MultiValue, Scope, Table};

use crate::model::{ModelBindOpts, ModelBinding, ModelResolver, ModelSet};
use crate::{Error, Result};

mod decode;
mod userdata;

pub(crate) use userdata::{LuaModelHandle, ModelInferHook, ModelsInferHook};

use decode::{parse_bind_args, parse_single_alias, validate_alias};

/// Dispatches a `models.infer(prompt)` call through the executor-installed
/// [`ModelsInferHook`] app data.
///
/// Shared by the live H1 and H2 `models` tables; the hook carries everything
/// else (current-model resolution, gateway client, section identity). The
/// call runs the one infer shape: a single tool-free round on a fresh
/// conversation that never sets `reply` or touches `sys`.
fn call_models_infer_hook(lua: &Lua, prompt: &str) -> mlua::Result<String> {
    let hook = lua
        .app_data_ref::<ModelsInferHook>()
        .ok_or_else(|| {
            mlua::Error::external("models.infer is not available outside section execution")
        })?
        .clone();
    hook(lua, prompt)
}

/// H2 model-recording state: wraps the at-most-once `models.use` selection.
#[derive(Debug)]
pub(crate) struct ModelRuntime {
    used: Option<String>,
}

impl ModelRuntime {
    pub(crate) fn new() -> Self {
        ModelRuntime { used: None }
    }

    /// The current `models.use` selection, if any.
    pub(crate) fn used(&self) -> Option<&str> {
        self.used.as_deref()
    }

    /// Records a `models.use` selection.
    ///
    /// # Errors
    /// Returns [`SelectError::AlreadyUsed`] if a selection was already recorded.
    pub(crate) fn select(&mut self, alias: String) -> std::result::Result<(), SelectError> {
        if self.used.is_some() {
            Err(SelectError::AlreadyUsed)
        } else {
            self.used = Some(alias);
            Ok(())
        }
    }
}

/// Why a `models.use` selection was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectError {
    /// A `models.use` selection was already recorded this section.
    AlreadyUsed,
}

/// Records the first concrete callback error, preserving its typed cause.
///
/// The error slot lives outside the shared [`ModelSet`] (the run context
/// reads that allocation through its view), so a poisoned set lock can never
/// swallow the typed resolution failure the H1 executor reports.
fn record_callback_error(errors: &Mutex<Option<Error>>, error: Error) -> mlua::Result<()> {
    let mut slot = errors
        .lock()
        .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
    if slot.is_none() {
        *slot = Some(error);
    }
    Ok(())
}

/// Records one `models.bind` binding into the shared set. Shared by
/// `models.bind` and the multi-arg `models.default` form.
fn record_bind_binding(
    set: &mut ModelSet,
    errors: &Mutex<Option<Error>>,
    resolver: &dyn ModelResolver,
    alias: &str,
    description: &str,
    opts: &ModelBindOpts,
) -> mlua::Result<ModelBinding> {
    if set.bindings.iter().any(|b| b.alias() == alias) {
        record_callback_error(
            errors,
            Error::DuplicateModelAlias {
                alias: alias.to_owned(),
            },
        )?;
        return Err(mlua::Error::external("duplicate model alias"));
    }
    let selection = match resolver.resolve(description, opts) {
        Ok(sel) => sel,
        Err(error) => {
            record_callback_error(errors, error)?;
            return Err(mlua::Error::external("model capability resolution failed"));
        }
    };
    let binding = ModelBinding::new(
        alias,
        description,
        selection.id,
        selection.invocation,
        selection.context,
    );
    set.bindings.push(binding.clone());
    Ok(binding)
}

/// Records a `models.default` selection, enforcing at-most-once.
fn record_default_selection(set: &mut ModelSet, alias: String) -> mlua::Result<()> {
    if set.default.is_some() {
        return Err(mlua::Error::external(
            "models.default may be called at most once per prompt",
        ));
    }
    set.default = Some(alias);
    Ok(())
}

/// Records the multi-argument `models.default(alias, description, opts)` form
/// atomically.
///
/// All preconditions (the at-most-once `default` rule and, via
/// [`record_bind_binding`], the duplicate-alias and resolution rules) are
/// checked BEFORE any state is mutated, so a rejected call can never leave a
/// half-recorded binding with no matching default alias behind. Only when every
/// precondition passes are the binding and the default alias committed together.
fn record_default_binding(
    set: &mut ModelSet,
    errors: &Mutex<Option<Error>>,
    resolver: &dyn ModelResolver,
    alias: &str,
    description: &str,
    opts: &ModelBindOpts,
) -> mlua::Result<ModelBinding> {
    if set.default.is_some() {
        return Err(mlua::Error::external(
            "models.default may be called at most once per prompt",
        ));
    }
    // `record_bind_binding` only pushes after its own preconditions pass, and we
    // have already verified `default` is unset, so this commit is atomic.
    let binding = record_bind_binding(set, errors, resolver, alias, description, opts)?;
    set.default = Some(alias.to_owned());
    Ok(binding)
}

/// Installs live H1 `models.bind` / `models.default` resolvers and
/// `models.infer`.
///
/// Each call resolves immediately and records the resulting frozen binding
/// into the run's shared [`ModelSet`] - the same allocation the run context
/// reads through its `ModelView`. `models.use` remains unavailable until
/// section execution. `models.infer` dispatches through the
/// executor-installed hook, which resolves the current model from the shared
/// set.
pub(crate) fn install_live_models<'scope, 'env: 'scope>(
    lua: &'env Lua,
    scope: &'scope Scope<'scope, 'env>,
    resolver: &'env dyn ModelResolver,
    set: &Arc<Mutex<ModelSet>>,
    errors: &Arc<Mutex<Option<Error>>>,
) -> Result<()> {
    let models = lua.create_table().map_err(Error::lua)?;

    let bind_set = Arc::clone(set);
    let bind_errors = Arc::clone(errors);
    let bind = scope
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
            let (alias, description, opts) = parse_bind_args(args, "models.bind")?;
            validate_alias(&alias).map_err(mlua::Error::external)?;
            let mut guard = bind_set
                .lock()
                .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
            let binding = record_bind_binding(
                &mut guard,
                &bind_errors,
                resolver,
                &alias,
                &description,
                &opts,
            )?;
            Ok(LuaModelHandle::from_binding(&binding))
        })
        .map_err(Error::lua)?;
    models.set("bind", bind).map_err(Error::lua)?;

    let default_set = Arc::clone(set);
    let default_errors = Arc::clone(errors);
    let default = scope
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
            if args.len() >= 2 {
                let (alias, description, opts) = parse_bind_args(args, "models.default")?;
                validate_alias(&alias).map_err(mlua::Error::external)?;
                let mut guard = default_set
                    .lock()
                    .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
                let binding = record_default_binding(
                    &mut guard,
                    &default_errors,
                    resolver,
                    &alias,
                    &description,
                    &opts,
                )?;
                Ok(LuaModelHandle::from_binding(&binding))
            } else {
                let alias = parse_single_alias(&args, "models.default")?;
                validate_alias(&alias).map_err(mlua::Error::external)?;
                let mut guard = default_set
                    .lock()
                    .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
                let binding = guard
                    .bindings
                    .iter()
                    .find(|b| b.alias() == alias)
                    .cloned()
                    .ok_or_else(|| {
                        mlua::Error::external(format!(
                            "models.default alias {alias:?} was not declared by models.bind"
                        ))
                    })?;
                record_default_selection(&mut guard, alias)?;
                Ok(LuaModelHandle::from_binding(&binding))
            }
        })
        .map_err(Error::lua)?;
    models.set("default", default).map_err(Error::lua)?;

    let use_fn = scope
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.use is only available during H2 recording",
            ))
        })
        .map_err(Error::lua)?;
    models.set("use", use_fn).map_err(Error::lua)?;

    let infer = scope
        .create_function(|lua, prompt: String| call_models_infer_hook(lua, &prompt))
        .map_err(Error::lua)?;
    models.set("infer", infer).map_err(Error::lua)?;

    lua.globals().raw_set("models", models).map_err(Error::lua)
}

/// Switches to H2: forbids `models.bind`, installs `models.use`,
/// `models.get`, and `models.infer`.
pub(crate) fn install_h2_models(
    lua: &Lua,
    globals: &Table,
    bindings: &ModelSet,
    runtime: &Arc<Mutex<ModelRuntime>>,
) -> Result<()> {
    let models = lua.create_table().map_err(Error::lua)?;

    let bind = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.bind is only available during live H1 execution",
            ))
        })
        .map_err(Error::lua)?;
    models.set("bind", bind).map_err(Error::lua)?;

    let default_fn = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.default is only available during live H1 execution",
            ))
        })
        .map_err(Error::lua)?;
    models.set("default", default_fn).map_err(Error::lua)?;

    let frozen = bindings.clone();
    let state = Arc::clone(runtime);
    let use_fn = lua
        .create_function(move |_, alias: String| -> mlua::Result<LuaModelHandle> {
            validate_alias(&alias).map_err(mlua::Error::external)?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("model declaration runtime was poisoned"))?;
            let binding = frozen.binding(&alias).cloned().ok_or_else(|| {
                mlua::Error::external(format!(
                    "models.use alias {alias:?} was not declared by models.bind"
                ))
            })?;
            state.select(alias).map_err(|_| {
                mlua::Error::external("models.use may be called at most once per section")
            })?;
            Ok(LuaModelHandle::from_binding(&binding))
        })
        .map_err(Error::lua)?;
    models.set("use", use_fn).map_err(Error::lua)?;

    let frozen = bindings.clone();
    let get_fn = lua
        .create_function(move |_, alias: String| -> mlua::Result<LuaModelHandle> {
            validate_alias(&alias).map_err(mlua::Error::external)?;
            let binding = frozen.binding(&alias).cloned().ok_or_else(|| {
                mlua::Error::external(format!(
                    "models.get alias {alias:?} was not declared by models.bind"
                ))
            })?;
            Ok(LuaModelHandle::from_binding(&binding))
        })
        .map_err(Error::lua)?;
    models.set("get", get_fn).map_err(Error::lua)?;

    let infer = lua
        .create_function(|lua, prompt: String| call_models_infer_hook(lua, &prompt))
        .map_err(Error::lua)?;
    models.set("infer", infer).map_err(Error::lua)?;

    globals.raw_set("models", models).map_err(Error::lua)
}

#[cfg(test)]
mod tests;
