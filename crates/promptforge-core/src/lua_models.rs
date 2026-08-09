//! Lua `models.need` / `models.use` host tables for H1 bind, replay, and H2.
//!
//! Kept beside [`crate::lua`] so the tool tables stay readable while model
//! declaration recording mirrors their phase rules.

use std::sync::Arc;
use std::sync::Mutex;

use mlua::{Lua, MultiValue, Scope, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::model::{ModelBinding, ModelBindings, ModelDeclaration, ModelNeedOpts, ModelResolver};
use crate::observe::{Observer, detail};
use crate::{Error, Result};

/// Host hook that runs `model:infer` from Lua via the executor's shared context.
///
/// Installed as Lua app data for the duration of a section phase that may call
/// infer. Absent app data means infer is unavailable in that context.
pub type ModelInferHook =
    Arc<dyn Fn(&Lua, &ModelBinding, &str) -> mlua::Result<String> + Send + Sync>;

/// Inspectable Lua userdata returned by `models.need` / `models.always`.
#[derive(Debug, Clone)]
pub struct LuaModelHandle {
    binding: ModelBinding,
}

impl LuaModelHandle {
    /// Builds a handle from a frozen [`ModelBinding`].
    #[must_use]
    pub fn from_binding(binding: &ModelBinding) -> Self {
        Self {
            binding: binding.clone(),
        }
    }

    /// Returns the frozen binding carried by this handle.
    #[must_use]
    pub fn binding(&self) -> &ModelBinding {
        &self.binding
    }

    /// Returns the prompt-local alias.
    #[must_use]
    pub fn name(&self) -> &str {
        self.binding.alias()
    }

    /// Returns the caller-facing catalog model id.
    #[must_use]
    pub fn model_id(&self) -> &str {
        self.binding.id().name()
    }

    /// Returns the capability description supplied to `models.need`.
    #[must_use]
    pub fn description(&self) -> &str {
        self.binding.description()
    }

    /// Returns the catalog context window size in tokens.
    #[must_use]
    pub fn context(&self) -> u32 {
        self.binding.context()
    }

    /// Returns the frozen thinking switch, when the need declared one.
    #[must_use]
    pub fn thinking(&self) -> Option<bool> {
        self.binding.invocation().thinking
    }

    /// Returns the frozen sampling temperature, when the need declared one.
    #[must_use]
    pub fn temperature(&self) -> Option<f64> {
        self.binding.invocation().temperature
    }

    /// Returns the frozen max generation tokens, when the need declared one.
    #[must_use]
    pub fn max_tokens(&self) -> Option<u32> {
        self.binding.invocation().max_tokens
    }

    /// Returns the tool-calling dialect id string.
    #[must_use]
    pub fn dialect(&self) -> String {
        self.binding.tool_dialect().to_string()
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
        fields.add_field_method_get("dialect", |_, this| Ok(this.dialect()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method(
            "infer",
            |lua, this, (prompt, _opts): (String, Option<Value>)| {
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
    pub(crate) always: Option<String>,
    pub(crate) callback_error: Option<Error>,
}

/// Records one `models.need` binding into the accumulator. Shared by
/// `models.need` and the multi-arg `models.always` form.
fn record_need_binding(
    state: &mut ModelBindingState,
    resolver: &dyn ModelResolver,
    alias: String,
    description: String,
    opts: ModelNeedOpts,
) -> mlua::Result<ModelBinding> {
    if state.bindings.iter().any(|b| b.alias() == alias) {
        if state.callback_error.is_none() {
            state.callback_error = Some(Error::DuplicateModelAlias {
                alias: alias.clone(),
            });
        }
        return Err(mlua::Error::external("duplicate model alias"));
    }
    let selection = match resolver.resolve(&description, &opts) {
        Ok(sel) => sel,
        Err(error) => {
            if state.callback_error.is_none() {
                state.callback_error = Some(error);
            }
            return Err(mlua::Error::external("model capability resolution failed"));
        }
    };
    let binding = ModelBinding::new(
        alias.clone(),
        description.clone(),
        selection.id,
        selection.invocation,
    )
    .with_dialect(selection.tool_dialect)
    .with_context(selection.context);
    state.bindings.push(binding.clone());
    state.declarations.push(ModelDeclaration::Need {
        alias,
        description,
        opts,
    });
    Ok(binding)
}

/// Records a `models.always` selection, enforcing at-most-once.
fn record_always_selection(state: &mut ModelBindingState, alias: String) -> mlua::Result<()> {
    if state.always.is_some() {
        return Err(mlua::Error::external(
            "models.always may be called at most once per prompt",
        ));
    }
    state.always = Some(alias.clone());
    state.declarations.push(ModelDeclaration::Always(alias));
    Ok(())
}

/// Extracts a single string alias from a `MultiValue` (for the 1-arg form).
fn parse_single_alias(args: &MultiValue, label: &str) -> mlua::Result<String> {
    match args.iter().next() {
        Some(Value::String(value)) => value
            .to_str()
            .map_err(|_| mlua::Error::external(format!("{label} alias must be a UTF-8 string")))
            .map(|s| s.to_owned()),
        _ => Err(mlua::Error::external(format!(
            "{label} expects a string alias as first argument"
        ))),
    }
}

/// Replays one declaration against the bound sequence, failing on mismatch.
fn replay_declaration(
    bindings: &ModelBindings,
    runtime: &mut ModelRuntime,
    expected: &ModelDeclaration,
    label: &str,
) -> mlua::Result<()> {
    let Some(declaration) = bindings.declarations().get(runtime.declaration_index) else {
        return Err(mlua::Error::external(format!(
            "{label} call has no matching bound declaration; bind the prompt before executing"
        )));
    };
    if declaration != expected {
        return Err(mlua::Error::external(format!(
            "model declaration replay mismatch at declaration {}",
            runtime.declaration_index + 1
        )));
    }
    runtime.declaration_index += 1;
    Ok(())
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
        .map_err(|error| Error::Lua(error.to_string()))?;

    let needs = Arc::clone(state);
    let need = scope
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
            let (alias, description, opts) = parse_need_args(args)?;
            validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
            let mut guard = needs
                .lock()
                .map_err(|_| mlua::Error::external("model binding recorder was poisoned"))?;
            let binding = record_need_binding(&mut guard, resolver, alias, description, opts)?;
            Ok(LuaModelHandle::from_binding(&binding))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("need", need)
        .map_err(|error| Error::Lua(error.to_string()))?;

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
                    record_need_binding(&mut guard, resolver, alias.clone(), description, opts)?;
                record_always_selection(&mut guard, alias)?;
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
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("always", always)
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

/// Installs the legacy H1 declaration-mode model table.
pub(crate) fn install_bind_models<'scope, 'env: 'scope>(
    lua: &'env Lua,
    scope: &'scope Scope<'scope, 'env>,
    resolver: &'env dyn ModelResolver,
    state: &Arc<Mutex<ModelBindingState>>,
) -> Result<()> {
    install_live_models(lua, scope, resolver, state)
}

/// Installs exact-replay `models.need`, `models.always`, and forbidden `models.use`.
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
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
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
            let expected_decl = ModelDeclaration::Need {
                alias: alias.clone(),
                description,
                opts,
            };
            replay_declaration(&expected, &mut state, &expected_decl, "models.need")?;
            let binding = expected.binding(&alias).ok_or_else(|| {
                mlua::Error::external(format!("models.need alias {alias:?} has no frozen binding"))
            })?;
            Ok(LuaModelHandle::from_binding(binding))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("need", need)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let expected_always = bindings.clone();
    let always_state = Arc::clone(runtime);
    let always = lua
        .create_function(move |_, args: MultiValue| -> mlua::Result<LuaModelHandle> {
            let mut state = always_state
                .lock()
                .map_err(|_| mlua::Error::external("model declaration runtime was poisoned"))?;
            if state.phase != ModelPhase::Replay {
                return Err(mlua::Error::external(
                    "models.always is only available during H1 binding or replay",
                ));
            }
            if args.len() >= 2 {
                let (alias, description, opts) = parse_need_args(args)?;
                validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
                let need_decl = ModelDeclaration::Need {
                    alias: alias.clone(),
                    description,
                    opts,
                };
                replay_declaration(&expected_always, &mut state, &need_decl, "models.always")?;
                let always_decl = ModelDeclaration::Always(alias.clone());
                replay_declaration(&expected_always, &mut state, &always_decl, "models.always")?;
                let binding = expected_always.binding(&alias).ok_or_else(|| {
                    mlua::Error::external(format!(
                        "models.always alias {alias:?} has no frozen binding"
                    ))
                })?;
                Ok(LuaModelHandle::from_binding(binding))
            } else {
                let alias = parse_single_alias(&args, "models.always")?;
                validate_alias(&alias).map_err(|error| mlua::Error::external(error.to_string()))?;
                let always_decl = ModelDeclaration::Always(alias.clone());
                replay_declaration(&expected_always, &mut state, &always_decl, "models.always")?;
                let binding = expected_always.binding(&alias).ok_or_else(|| {
                    mlua::Error::external(format!(
                        "models.always alias {alias:?} has no frozen binding"
                    ))
                })?;
                Ok(LuaModelHandle::from_binding(binding))
            }
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("always", always)
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

    let always_fn = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.always is only available during H1 binding or replay",
            ))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("always", always_fn)
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
    let effective_alias = runtime
        .used
        .clone()
        .or_else(|| bindings.always().map(String::from));
    match effective_alias {
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
