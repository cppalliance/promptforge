//! Lua `models.need` / `models.use` host tables for live H1 and H2.
//!
//! Kept beside [`crate::lua`] so the tool tables stay readable while model
//! declaration recording mirrors their phase rules.

use std::sync::Arc;
use std::sync::Mutex;

use mlua::{Lua, MultiValue, Scope, Table};

use crate::model::{ModelBinding, ModelBindings, ModelNeedOpts, ModelResolver};
use crate::observe::{Observer, detail};
use crate::{Error, Result};

mod decode;
mod userdata;

pub(crate) use userdata::{LuaModelHandle, ModelInferHook};

use decode::{parse_need_args, parse_single_alias, validate_alias};

/// H2 model-recording state, as phase-owning variants (PF-LM-006).
///
/// A `models.use` selection is only valid while recording is open, so it lives
/// inside the `Open` variant; a `Closed` scope owns no selection state, making a
/// "used-after-close" mutation unrepresentable. Fields are private; all access is
/// through the methods below.
#[derive(Debug)]
pub(crate) enum ModelRuntime {
    /// H2 recording is open; `used` holds an optional `models.use` selection.
    Open { used: Option<String> },
    /// Recording has closed; no further selection or close is possible.
    Closed,
}

impl ModelRuntime {
    pub(crate) fn new() -> Self {
        ModelRuntime::Open { used: None }
    }

    /// Whether recording is still open.
    pub(crate) fn is_open(&self) -> bool {
        matches!(self, ModelRuntime::Open { .. })
    }

    /// The current `models.use` selection, if any.
    pub(crate) fn used(&self) -> Option<&str> {
        match self {
            ModelRuntime::Open { used } => used.as_deref(),
            ModelRuntime::Closed => None,
        }
    }

    /// Records a `models.use` selection.
    ///
    /// # Errors
    /// Returns [`SelectError::Closed`] if recording has closed, or
    /// [`SelectError::AlreadyUsed`] if a selection was already recorded.
    pub(crate) fn select(&mut self, alias: String) -> std::result::Result<(), SelectError> {
        match self {
            ModelRuntime::Open { used } if used.is_some() => Err(SelectError::AlreadyUsed),
            ModelRuntime::Open { used } => {
                *used = Some(alias);
                Ok(())
            }
            ModelRuntime::Closed => Err(SelectError::Closed),
        }
    }

    /// Transitions an open scope to closed. Idempotent-safe: a `Closed` scope
    /// stays closed.
    pub(crate) fn close(&mut self) {
        *self = ModelRuntime::Closed;
    }
}

/// Why a `models.use` selection was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectError {
    /// The model scope had already closed.
    Closed,
    /// A `models.use` selection was already recorded this section.
    AlreadyUsed,
}

/// Accumulator populated by model needs executed during live H1.
#[derive(Debug, Default)]
pub(crate) struct ModelBindingState {
    pub(crate) bindings: Vec<ModelBinding>,
    pub(crate) always: Option<String>,
    pub(crate) callback_error: Option<Error>,
}

/// Records one `models.need` binding into the accumulator. Shared by
/// `models.need` and the multi-arg `models.always` form.
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

/// Records a `models.always` selection, enforcing at-most-once.
fn record_always_selection(state: &mut ModelBindingState, alias: String) -> mlua::Result<()> {
    if state.always.is_some() {
        return Err(mlua::Error::external(
            "models.always may be called at most once per prompt",
        ));
    }
    state.always = Some(alias);
    Ok(())
}

/// Records the multi-argument `models.always(alias, description, opts)` form
/// atomically.
///
/// All preconditions (the at-most-once `always` rule and, via
/// [`record_need_binding`], the duplicate-alias and resolution rules) are
/// checked BEFORE any state is mutated, so a rejected call can never leave a
/// half-recorded binding with no matching default alias behind. Only when every
/// precondition passes are the binding and the default alias committed together.
fn record_always_binding(
    state: &mut ModelBindingState,
    resolver: &dyn ModelResolver,
    alias: &str,
    description: &str,
    opts: &ModelNeedOpts,
) -> mlua::Result<ModelBinding> {
    if state.always.is_some() {
        return Err(mlua::Error::external(
            "models.always may be called at most once per prompt",
        ));
    }
    // `record_need_binding` only pushes after its own preconditions pass, and we
    // have already verified `always` is unset, so this commit is atomic.
    let binding = record_need_binding(state, resolver, alias, description, opts)?;
    state.always = Some(alias.to_owned());
    Ok(binding)
}

/// Installs live H1 `models.need` / `models.always` resolvers.
///
/// Each call resolves immediately and records the resulting frozen binding.
/// `models.use` remains unavailable until section execution.
pub(crate) fn install_live_models<'scope, 'env: 'scope>(
    lua: &'env Lua,
    scope: &'scope Scope<'scope, 'env>,
    resolver: &'env dyn ModelResolver,
    state: &Arc<Mutex<ModelBindingState>>,
) -> Result<()> {
    let models = lua
        .create_table()
        .map_err(Error::lua)?;

    let needs = Arc::clone(state);
    let need = scope
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
            let (alias, description, opts) = parse_need_args(args)?;
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut guard = needs
                .lock()
                .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
            let binding = record_need_binding(&mut guard, resolver, &alias, &description, &opts)?;
            Ok(LuaModelHandle::from_binding(&binding))
        })
        .map_err(Error::lua)?;
    models
        .set("need", need)
        .map_err(Error::lua)?;

    let always_state = Arc::clone(state);
    let always = scope
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
            if args.len() >= 2 {
                let (alias, description, opts) = parse_need_args(args)?;
                validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
                let mut guard = always_state
                    .lock()
                    .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
                let binding =
                    record_always_binding(&mut guard, resolver, &alias, &description, &opts)?;
                Ok(LuaModelHandle::from_binding(&binding))
            } else {
                let alias = parse_single_alias(&args, "models.always")?;
                validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
                let mut guard = always_state
                    .lock()
                    .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
                let binding = guard
                    .bindings
                    .iter()
                    .find(|b| b.alias() == alias)
                    .cloned()
                    .ok_or_else(|| {
                        mlua::Error::external(format!(
                            "models.always alias {alias:?} was not declared by models.need"
                        ))
                    })?;
                record_always_selection(&mut guard, alias)?;
                Ok(LuaModelHandle::from_binding(&binding))
            }
        })
        .map_err(Error::lua)?;
    models
        .set("always", always)
        .map_err(Error::lua)?;

    let use_fn = scope
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.use is only available during H2 recording",
            ))
        })
        .map_err(Error::lua)?;
    models
        .set("use", use_fn)
        .map_err(Error::lua)?;

    lua.globals()
        .raw_set("models", models)
        .map_err(Error::lua)
}

/// Switches to H2: forbids `models.need`, installs `models.use`.
pub(crate) fn install_h2_models(
    lua: &Lua,
    globals: &Table,
    bindings: &ModelBindings,
    runtime: &Arc<Mutex<ModelRuntime>>,
) -> Result<()> {
    {
        let state = runtime
            .lock()
            .map_err(|_| Error::Lua("model declaration runtime was poisoned".to_owned()))?;
        if !state.is_open() {
            return Err(Error::Lua(
                "model scope is not open for H2 recording".to_owned(),
            ));
        }
    }

    let models = lua
        .create_table()
        .map_err(Error::lua)?;

    let need = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.need is only available during live H1 execution",
            ))
        })
        .map_err(Error::lua)?;
    models
        .set("need", need)
        .map_err(Error::lua)?;

    let always_fn = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.always is only available during live H1 execution",
            ))
        })
        .map_err(Error::lua)?;
    models
        .set("always", always_fn)
        .map_err(Error::lua)?;

    let frozen = bindings.clone();
    let state = Arc::clone(runtime);
    let use_fn = lua
        .create_function(move |_, alias: String| {
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("model declaration runtime was poisoned"))?;
            if !state.is_open() {
                return Err(mlua::Error::external(
                    "models.use is only available before the H2 model scope closes",
                ));
            }
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
    models
        .set("use", use_fn)
        .map_err(Error::lua)?;

    globals
        .raw_set("models", models)
        .map_err(Error::lua)
}

/// Closes H2 model recording and returns the section's selected binding.
pub(crate) fn close_model_scope(
    bindings: &ModelBindings,
    runtime: &Arc<Mutex<ModelRuntime>>,
    execution: &str,
    observer: &dyn Observer,
    section: &str,
) -> Result<Option<ModelBinding>> {
    observer.observe(execution, section, detail::MODEL_SCOPE_CLOSING);
    let result = close_model_scope_inner(bindings, runtime);
    observer.observe(
        execution,
        section,
        if result.is_ok() {
            detail::MODEL_SCOPE_CLOSED
        } else {
            detail::MODEL_SCOPE_FAILED
        },
    );
    result
}

fn close_model_scope_inner(
    bindings: &ModelBindings,
    runtime: &Arc<Mutex<ModelRuntime>>,
) -> Result<Option<ModelBinding>> {
    let mut runtime = runtime
        .lock()
        .map_err(|_| Error::Lua("model declaration runtime was poisoned".to_owned()))?;
    if !runtime.is_open() {
        return Err(Error::Lua(
            "model scope can only close once after H2 recording".to_owned(),
        ));
    }
    // Resolve (and clone) the effective binding BEFORE transitioning to Closed,
    // so a missing frozen binding fails while the scope is still H2. Otherwise a
    // failed close would leave a Closed scope whose selected alias has no
    // binding - an inconsistent state. The close below is infallible.
    let effective_alias = runtime
        .used()
        .map(String::from)
        .or_else(|| bindings.always().map(String::from));
    let resolved =
        match effective_alias {
            Some(alias) => Some(bindings.binding(&alias).cloned().ok_or_else(|| {
                Error::Lua(format!("model alias {alias:?} has no frozen binding"))
            })?),
            None => None,
        };
    runtime.close();
    Ok(resolved)
}

#[cfg(test)]
mod tests;
