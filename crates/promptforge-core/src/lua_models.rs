//! Lua `models.need` / `models.use` host tables for H1 bind, replay, and H2.
//!
//! Kept beside [`crate::lua`] so the tool tables stay readable while model
//! declaration recording mirrors their phase rules.

use std::sync::Arc;
use std::sync::Mutex;

use mlua::{Lua, MultiValue, Scope, Table, Value};

use crate::model::{
    ModelBinding, ModelBindings, ModelDeclaration, ModelNeedOpts, ModelResolver, ResolvedModel,
};
use crate::observe::{Observer, detail};
use crate::{Error, Result};

/// Phase of the section-local models table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelPhase {
    Replay,
    H2,
    Closed,
}

/// Mutable H2 recording state for `models.use`.
#[derive(Debug)]
pub(crate) struct ModelRuntime {
    pub(crate) phase: ModelPhase,
    pub(crate) declaration_index: usize,
    pub(crate) used: Option<String>,
}

impl ModelRuntime {
    pub(crate) fn new_replay() -> Self {
        Self {
            phase: ModelPhase::Replay,
            declaration_index: 0,
            used: None,
        }
    }
}

/// Accumulator for one H1 model declaration pass.
#[derive(Debug, Default)]
pub(crate) struct ModelBindingState {
    pub(crate) bindings: Vec<ModelBinding>,
    pub(crate) declarations: Vec<ModelDeclaration>,
    pub(crate) callback_error: Option<Error>,
}

/// Installs H1 binding-mode `models.need` / forbidden `models.use`.
pub(crate) fn install_bind_models<'scope, 'env: 'scope>(
    lua: &'env Lua,
    scope: &'scope Scope<'scope, 'env>,
    resolver: &'env dyn ModelResolver,
    state: &Arc<Mutex<ModelBindingState>>,
) -> Result<()> {
    let models = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let needs = Arc::clone(state);
    let need = scope
        .create_function(move |_, args: MultiValue| -> mlua::Result<()> {
            let (alias, description, opts) = parse_need_args(args)?;
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut declarations = needs
                .lock()
                .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
            if declarations
                .bindings
                .iter()
                .any(|binding| binding.alias() == alias)
            {
                if declarations.callback_error.is_none() {
                    declarations.callback_error = Some(Error::DuplicateModelAlias {
                        alias: alias.clone(),
                    });
                }
                return Err(mlua::Error::external("duplicate model alias"));
            }
            let ResolvedModel { id, invocation } = match resolver.resolve(&description, &opts) {
                Ok(selection) => selection,
                Err(error) => {
                    if declarations.callback_error.is_none() {
                        declarations.callback_error = Some(error);
                    }
                    return Err(mlua::Error::external("model capability resolution failed"));
                }
            };
            declarations.bindings.push(ModelBinding::new(
                alias.clone(),
                description.clone(),
                id,
                invocation,
            ));
            declarations.declarations.push(ModelDeclaration::Need {
                alias,
                description,
                opts,
            });
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("need", need)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let use_fn = scope
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.use is only available during H2 recording",
            ))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("use", use_fn)
        .map_err(|error| Error::Lua(error.to_string()))?;

    lua.globals()
        .raw_set("models", models)
        .map_err(|error| Error::Lua(error.to_string()))
}

/// Installs exact-replay `models.need` and forbidden `models.use`.
pub(crate) fn install_replay_models(
    lua: &Lua,
    bindings: &ModelBindings,
    runtime: &Arc<Mutex<ModelRuntime>>,
) -> Result<()> {
    let models = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let expected = bindings.clone();
    let state = Arc::clone(runtime);
    let need = lua
        .create_function(move |_, args: MultiValue| {
            let (alias, description, opts) = parse_need_args(args)?;
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("model declaration runtime was poisoned"))?;
            if state.phase != ModelPhase::Replay {
                return Err(mlua::Error::external(
                    "models.need is only available during H1 binding or replay",
                ));
            }
            let Some(declaration) = expected.declarations().get(state.declaration_index) else {
                return Err(mlua::Error::external(
                    "models.need call has no matching bound declaration; bind the prompt before executing",
                ));
            };
            let expected_decl = ModelDeclaration::Need {
                alias,
                description,
                opts,
            };
            if declaration != &expected_decl {
                return Err(mlua::Error::external(format!(
                    "model declaration replay mismatch at declaration {}",
                    state.declaration_index + 1
                )));
            }
            state.declaration_index += 1;
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("need", need)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let use_fn = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.use is only available during H2 recording",
            ))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("use", use_fn)
        .map_err(|error| Error::Lua(error.to_string()))?;

    lua.globals()
        .raw_set("models", models)
        .map_err(|error| Error::Lua(error.to_string()))
}

/// Switches to H2: forbids `models.need`, installs `models.use`.
pub(crate) fn install_h2_models(
    lua: &Lua,
    globals: &Table,
    bindings: &ModelBindings,
    runtime: &Arc<Mutex<ModelRuntime>>,
) -> Result<()> {
    {
        let mut state = runtime
            .lock()
            .map_err(|_| Error::Lua("model declaration runtime was poisoned".to_owned()))?;
        if state.phase != ModelPhase::Replay {
            return Err(Error::Lua(
                "model declaration runtime did not finish replay".to_owned(),
            ));
        }
        state.phase = ModelPhase::H2;
    }

    let models = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let need = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.need is only available during H1 binding or replay",
            ))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("need", need)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let frozen = bindings.clone();
    let state = Arc::clone(runtime);
    let use_fn = lua
        .create_function(move |_, alias: String| {
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("model declaration runtime was poisoned"))?;
            if state.phase != ModelPhase::H2 {
                return Err(mlua::Error::external(
                    "models.use is only available before the H2 model scope closes",
                ));
            }
            if frozen.binding(&alias).is_none() {
                return Err(mlua::Error::external(format!(
                    "models.use alias {alias:?} was not declared by models.need"
                )));
            }
            if state.used.is_some() {
                return Err(mlua::Error::external(
                    "models.use may be called at most once per section",
                ));
            }
            state.used = Some(alias);
            Ok(())
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("use", use_fn)
        .map_err(|error| Error::Lua(error.to_string()))?;

    globals
        .raw_set("models", models)
        .map_err(|error| Error::Lua(error.to_string()))
}

/// Finishes model declaration replay (all declarations consumed).
pub(crate) fn finish_model_replay(bindings: &ModelBindings, runtime: &ModelRuntime) -> Result<()> {
    if runtime.declaration_index != bindings.declarations().len() {
        return Err(Error::Lua(format!(
            "model declaration replay ended after {}/{} declarations",
            runtime.declaration_index,
            bindings.declarations().len()
        )));
    }
    Ok(())
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
    if runtime.phase != ModelPhase::H2 {
        return Err(Error::Lua(
            "model scope can only close once after H2 recording".to_owned(),
        ));
    }
    runtime.phase = ModelPhase::Closed;
    match runtime.used.clone() {
        Some(alias) => {
            let binding = bindings.binding(&alias).cloned().ok_or_else(|| {
                Error::Lua(format!("model alias {alias:?} has no frozen binding"))
            })?;
            Ok(Some(binding))
        }
        None => Ok(None),
    }
}

fn parse_need_args(args: MultiValue) -> mlua::Result<(String, String, ModelNeedOpts)> {
    let mut values = args.into_iter();
    let alias = match values.next() {
        Some(Value::String(value)) => value
            .to_str()
            .map_err(|_| mlua::Error::external("models.need alias must be a UTF-8 string"))?
            .to_owned(),
        _ => {
            return Err(mlua::Error::external(
                "models.need expects alias, description, and optional opts table",
            ));
        }
    };
    let description = match values.next() {
        Some(Value::String(value)) => value
            .to_str()
            .map_err(|_| mlua::Error::external("models.need description must be a UTF-8 string"))?
            .to_owned(),
        _ => {
            return Err(mlua::Error::external(
                "models.need expects alias, description, and optional opts table",
            ));
        }
    };
    let opts = match values.next() {
        None | Some(Value::Nil) => ModelNeedOpts::default(),
        Some(Value::Table(table)) => parse_opts_table(&table)?,
        Some(_) => {
            return Err(mlua::Error::external(
                "models.need opts must be a table when provided",
            ));
        }
    };
    if values.next().is_some() {
        return Err(mlua::Error::external(
            "models.need expects at most three arguments",
        ));
    }
    Ok((alias, description, opts))
}

fn parse_opts_table(table: &Table) -> mlua::Result<ModelNeedOpts> {
    let mut opts = ModelNeedOpts::default();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|error| mlua::Error::external(error.to_string()))?;
        let key = match key {
            Value::String(key) => key
                .to_str()
                .map_err(|_| mlua::Error::external("models.need opts key must be a UTF-8 string"))?
                .to_owned(),
            _ => {
                return Err(mlua::Error::external(
                    "models.need opts keys must be strings",
                ));
            }
        };
        match key.as_str() {
            "thinking" => {
                opts.thinking = Some(value_as_bool(&value, "thinking")?);
            }
            "context" => {
                opts.context = Some(value_as_u32(&value, "context")?);
            }
            "temperature" => {
                opts.temperature = Some(value_as_f64(&value, "temperature")?);
            }
            "max_tokens" => {
                opts.max_tokens = Some(value_as_u32(&value, "max_tokens")?);
            }
            other => {
                return Err(mlua::Error::external(format!(
                    "unknown models.need opts key {other:?}"
                )));
            }
        }
    }
    Ok(opts)
}

fn value_as_bool(value: &Value, field: &str) -> mlua::Result<bool> {
    match value {
        Value::Boolean(flag) => Ok(*flag),
        _ => Err(mlua::Error::external(format!(
            "models.need opts.{field} must be a boolean"
        ))),
    }
}

fn value_as_u32(value: &Value, field: &str) -> mlua::Result<u32> {
    match value {
        Value::Integer(number) => u32::try_from(*number).map_err(|_| {
            mlua::Error::external(format!(
                "models.need opts.{field} must be a non-negative integer"
            ))
        }),
        Value::Number(number) if number.fract() == 0.0 => {
            let truncated = number.trunc();
            if (0.0..=f64::from(u32::MAX)).contains(&truncated) {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "range checked against u32::MAX and non-negative"
                )]
                Ok(truncated as u32)
            } else {
                Err(mlua::Error::external(format!(
                    "models.need opts.{field} must be a non-negative integer"
                )))
            }
        }
        _ => Err(mlua::Error::external(format!(
            "models.need opts.{field} must be a non-negative integer"
        ))),
    }
}

fn value_as_f64(value: &Value, field: &str) -> mlua::Result<f64> {
    match value {
        Value::Number(number) => Ok(*number),
        Value::Integer(number) => i32::try_from(*number).map(f64::from).map_err(|_| {
            mlua::Error::external(format!(
                "models.need opts.{field} must be a number within i32 range"
            ))
        }),
        _ => Err(mlua::Error::external(format!(
            "models.need opts.{field} must be a number"
        ))),
    }
}

fn validate_alias(alias: &str) -> Result<()> {
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
            "invalid model alias {alias:?}: expected [A-Za-z][A-Za-z0-9_-]{{0,63}}"
        )))
    }
}
