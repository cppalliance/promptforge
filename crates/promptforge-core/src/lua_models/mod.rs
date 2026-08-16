//! Lua `models.need` / `models.use` host tables for live H1 and H2.
//!
//! Kept beside [`crate::lua`] so the tool tables stay readable while model
//! declaration recording mirrors their phase rules.

use std::sync::Arc;
use std::sync::Mutex;

use mlua::{Lua, MultiValue, Scope, Table};

use crate::model::{ModelBinding, ModelBindings, ModelNeedOpts, ModelResolver};
use crate::{Error, Result};

mod decode;
mod userdata;

pub(crate) use userdata::{LuaModelHandle, ModelInferHook};

use decode::{parse_need_args, parse_single_alias, validate_alias};

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

/// Accumulator populated by model needs executed during live H1.
#[derive(Debug, Default)]
pub(crate) struct ModelBindingState {
    pub(crate) bindings: Vec<ModelBinding>,
    pub(crate) only: Option<String>,
    pub(crate) callback_error: Option<Error>,
}

/// Records one `models.need` binding into the accumulator. Shared by
/// `models.need` and the multi-arg `models.only` form.
fn record_need_binding(
    state: &mut ModelBindingState,
    resolver: &dyn ModelResolver,
    alias: &str,
    description: &str,
    opts: &ModelNeedOpts,
) -> mlua::Result<ModelBinding> {
    if state.bindings.iter().any(|b| b.alias() == alias) {
        if state.callback_error.is_none() {
            state.callback_error = Some(Error::DuplicateModelAlias {
                alias: alias.to_owned(),
            });
        }
        return Err(mlua::Error::external("duplicate model alias"));
    }
    let selection = match resolver.resolve(description, opts) {
        Ok(sel) => sel,
        Err(error) => {
            if state.callback_error.is_none() {
                state.callback_error = Some(error);
            }
            return Err(mlua::Error::external("model capability resolution failed"));
        }
    };
    let binding = ModelBinding::new(
        alias,
        description,
        selection.id,
        selection.invocation,
        selection.tool_dialect,
        selection.context,
    );
    state.bindings.push(binding.clone());
    Ok(binding)
}

/// Records a `models.only` selection, enforcing at-most-once.
fn record_only_selection(state: &mut ModelBindingState, alias: String) -> mlua::Result<()> {
    if state.only.is_some() {
        return Err(mlua::Error::external(
            "models.only may be called at most once per prompt",
        ));
    }
    state.only = Some(alias);
    Ok(())
}

/// Records the multi-argument `models.only(alias, description, opts)` form
/// atomically.
///
/// All preconditions (the at-most-once `only` rule and, via
/// [`record_need_binding`], the duplicate-alias and resolution rules) are
/// checked BEFORE any state is mutated, so a rejected call can never leave a
/// half-recorded binding with no matching default alias behind. Only when every
/// precondition passes are the binding and the default alias committed together.
fn record_only_binding(
    state: &mut ModelBindingState,
    resolver: &dyn ModelResolver,
    alias: &str,
    description: &str,
    opts: &ModelNeedOpts,
) -> mlua::Result<ModelBinding> {
    if state.only.is_some() {
        return Err(mlua::Error::external(
            "models.only may be called at most once per prompt",
        ));
    }
    // `record_need_binding` only pushes after its own preconditions pass, and we
    // have already verified `only` is unset, so this commit is atomic.
    let binding = record_need_binding(state, resolver, alias, description, opts)?;
    state.only = Some(alias.to_owned());
    Ok(binding)
}

/// Installs live H1 `models.need` / `models.only` resolvers.
///
/// Each call resolves immediately and records the resulting frozen binding.
/// `models.use` remains unavailable until section execution.
pub(crate) fn install_live_models<'scope, 'env: 'scope>(
    lua: &'env Lua,
    scope: &'scope Scope<'scope, 'env>,
    resolver: &'env dyn ModelResolver,
    state: &Arc<Mutex<ModelBindingState>>,
) -> Result<()> {
    let models = lua.create_table().map_err(Error::lua)?;

    let needs = Arc::clone(state);
    let need = scope
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
            let (alias, description, opts) = parse_need_args(args)?;
            validate_alias(&alias).map_err(mlua::Error::external)?;
            let mut guard = needs
                .lock()
                .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
            let binding = record_need_binding(&mut guard, resolver, &alias, &description, &opts)?;
            Ok(LuaModelHandle::from_binding(&binding))
        })
        .map_err(Error::lua)?;
    models.set("need", need).map_err(Error::lua)?;

    let only_state = Arc::clone(state);
    let only = scope
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
            if args.len() >= 2 {
                let (alias, description, opts) = parse_need_args(args)?;
                validate_alias(&alias).map_err(mlua::Error::external)?;
                let mut guard = only_state
                    .lock()
                    .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
                let binding =
                    record_only_binding(&mut guard, resolver, &alias, &description, &opts)?;
                Ok(LuaModelHandle::from_binding(&binding))
            } else {
                let alias = parse_single_alias(&args, "models.only")?;
                validate_alias(&alias).map_err(mlua::Error::external)?;
                let mut guard = only_state
                    .lock()
                    .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
                let binding = guard
                    .bindings
                    .iter()
                    .find(|b| b.alias() == alias)
                    .cloned()
                    .ok_or_else(|| {
                        mlua::Error::external(format!(
                            "models.only alias {alias:?} was not declared by models.need"
                        ))
                    })?;
                record_only_selection(&mut guard, alias)?;
                Ok(LuaModelHandle::from_binding(&binding))
            }
        })
        .map_err(Error::lua)?;
    models.set("only", only).map_err(Error::lua)?;

    let use_fn = scope
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.use is only available during H2 recording",
            ))
        })
        .map_err(Error::lua)?;
    models.set("use", use_fn).map_err(Error::lua)?;

    lua.globals().raw_set("models", models).map_err(Error::lua)
}

/// Switches to H2: forbids `models.need`, installs `models.use`.
pub(crate) fn install_h2_models(
    lua: &Lua,
    globals: &Table,
    bindings: &ModelBindings,
    runtime: &Arc<Mutex<ModelRuntime>>,
) -> Result<()> {
    let models = lua.create_table().map_err(Error::lua)?;

    let need = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.need is only available during live H1 execution",
            ))
        })
        .map_err(Error::lua)?;
    models.set("need", need).map_err(Error::lua)?;

    let only_fn = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.only is only available during live H1 execution",
            ))
        })
        .map_err(Error::lua)?;
    models.set("only", only_fn).map_err(Error::lua)?;

    if bindings.only().is_some() {
        let use_fn = lua
            .create_function(|_, _: String| -> mlua::Result<()> {
                Err(mlua::Error::external(
                    "models.use is unavailable: models.only was called in H1",
                ))
            })
            .map_err(Error::lua)?;
        models.set("use", use_fn).map_err(Error::lua)?;
    } else {
        let frozen = bindings.clone();
        let state = Arc::clone(runtime);
        let use_fn = lua
            .create_function(move |_, alias: String| {
                validate_alias(&alias).map_err(mlua::Error::external)?;
                let mut state = state
                    .lock()
                    .map_err(|_| mlua::Error::external("model declaration runtime was poisoned"))?;
                if frozen.binding(&alias).is_none() {
                    return Err(mlua::Error::external(format!(
                        "models.use alias {alias:?} was not declared by models.need"
                    )));
                }
                state.select(alias).map_err(|_| {
                    mlua::Error::external("models.use may be called at most once per section")
                })?;
                Ok(())
            })
            .map_err(Error::lua)?;
        models.set("use", use_fn).map_err(Error::lua)?;
    }

    globals.raw_set("models", models).map_err(Error::lua)
}

#[cfg(test)]
mod tests;
