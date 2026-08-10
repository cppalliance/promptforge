//! Lua `models.need` / `models.use` host tables for live H1 and H2.
//!
//! Kept beside [`crate::lua`] so the tool tables stay readable while model
//! declaration recording mirrors their phase rules.

use std::sync::Arc;
use std::sync::Mutex;

use mlua::{Lua, MultiValue, Scope, Table, UserData, UserDataFields, UserDataMethods, Value};

use crate::dialects::ToolDialectId;
use crate::model::{ModelBinding, ModelBindings, ModelNeedOpts, ModelResolver};
use crate::observe::{Observer, detail};
use crate::{Error, Result};

/// Host hook that runs `model:infer` from Lua via the executor's shared context.
///
/// Installed as Lua app data for the duration of a section phase that may call
/// infer. Absent app data means infer is unavailable in that context.
pub(crate) type ModelInferHook =
    Arc<dyn Fn(&Lua, &ModelBinding, &str) -> mlua::Result<String> + Send + Sync>;

/// Inspectable Lua userdata returned by `models.need` / `models.always`.
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
    #[must_use]
    pub(crate) fn context(&self) -> u32 {
        self.binding.context()
    }

    /// Returns the frozen thinking switch, when the need declared one.
    #[must_use]
    pub(crate) fn thinking(&self) -> Option<bool> {
        self.binding.invocation().thinking
    }

    /// Returns the frozen sampling temperature, when the need declared one.
    #[must_use]
    pub(crate) fn temperature(&self) -> Option<f64> {
        self.binding.invocation().temperature
    }

    /// Returns the frozen max generation tokens, when the need declared one.
    #[must_use]
    pub(crate) fn max_tokens(&self) -> Option<u32> {
        self.binding.invocation().max_tokens
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
fn reject_infer_options(opts: Option<&Value>) -> mlua::Result<()> {
    match opts {
        None | Some(Value::Nil) => Ok(()),
        Some(_) => Err(mlua::Error::external(
            "model:infer(prompt) does not accept a second argument; \
             per-call inference options are not supported",
        )),
    }
}

/// Phase of the section-local models table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelPhase {
    H2,
    Closed,
}

/// Mutable H2 recording state for `models.use`.
#[derive(Debug)]
pub(crate) struct ModelRuntime {
    pub(crate) phase: ModelPhase,
    pub(crate) used: Option<String>,
}

impl ModelRuntime {
    pub(crate) fn new() -> Self {
        Self {
            phase: ModelPhase::H2,
            used: None,
        }
    }
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
    let binding = ModelBinding::new(alias, description, selection.id, selection.invocation)
        .with_dialect(selection.tool_dialect)
        .with_context(selection.context);
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
            let binding = record_need_binding(&mut guard, resolver, &alias, &description, &opts)?;
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
        if state.phase != ModelPhase::H2 {
            return Err(Error::Lua(
                "model scope is not open for H2 recording".to_owned(),
            ));
        }
    }

    let models = lua
        .create_table()
        .map_err(|error| Error::Lua(error.to_string()))?;

    let need = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.need is only available during live H1 execution",
            ))
        })
        .map_err(|error| Error::Lua(error.to_string()))?;
    models
        .set("need", need)
        .map_err(|error| Error::Lua(error.to_string()))?;

    let always_fn = lua
        .create_function(|_, _: MultiValue| -> mlua::Result<()> {
            Err(mlua::Error::external(
                "models.always is only available during live H1 execution",
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
    // Resolve (and clone) the effective binding BEFORE transitioning to Closed,
    // so a missing frozen binding fails while the scope is still H2. Otherwise a
    // failed close would leave a Closed scope whose selected alias has no
    // binding - an inconsistent state. The phase write below is infallible.
    let effective_alias = runtime
        .used
        .clone()
        .or_else(|| bindings.always().map(String::from));
    let resolved =
        match effective_alias {
            Some(alias) => Some(bindings.binding(&alias).cloned().ok_or_else(|| {
                Error::Lua(format!("model alias {alias:?} has no frozen binding"))
            })?),
            None => None,
        };
    runtime.phase = ModelPhase::Closed;
    Ok(resolved)
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
                opts.temperature = Some(value_as_temperature(&value)?);
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

/// The inclusive domain of a sampling temperature.
const TEMPERATURE_RANGE: std::ops::RangeInclusive<f64> = 0.0..=2.0;

/// Parses and validates a sampling temperature at the Lua trust boundary.
///
/// Lua integer and number forms are decoded through ONE numeric path (no
/// arbitrary `i32` gate), then validated by ONE domain check: the result must be
/// a finite number within [`TEMPERATURE_RANGE`]. Non-finite (`NaN`, infinity) or
/// out-of-domain values are rejected here rather than forwarded to the gateway
/// as an invalid request.
fn value_as_temperature(value: &Value) -> mlua::Result<f64> {
    let temperature = decode_lua_number(value, "temperature")?;
    if !temperature.is_finite() || !TEMPERATURE_RANGE.contains(&temperature) {
        return Err(mlua::Error::external(format!(
            "models.need opts.temperature must be a finite number in [0.0, 2.0], got {temperature}"
        )));
    }
    Ok(temperature)
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
                #[expect(
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

/// Decodes a Lua integer or number into an `f64` through a single path.
///
/// Both Lua numeric forms are accepted and converted uniformly; the caller's
/// domain check (for example [`value_as_temperature`]) is the single place that
/// bounds the result, so there is no separate, arbitrary integer-range gate.
fn decode_lua_number(value: &Value, field: &str) -> mlua::Result<f64> {
    match value {
        Value::Number(number) => Ok(*number),
        Value::Integer(number) => {
            // Lua integers are i64. The caller bounds the domain (temperatures
            // live in [0.0, 2.0]); any magnitude that would lose precision here
            // is far outside that domain and rejected by the caller's check.
            #[expect(
                clippy::cast_precision_loss,
                reason = "domain is bounded by the caller; large magnitudes are rejected there"
            )]
            Ok(*number as f64)
        }
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{
        LuaModelHandle, ModelBindingState, ModelPhase, ModelRuntime, close_model_scope_inner,
        decode_lua_number, parse_need_args, parse_opts_table, parse_single_alias,
        record_always_binding, reject_infer_options, validate_alias, value_as_bool,
        value_as_temperature, value_as_u32,
    };
    use crate::dialects::ToolDialectId;
    use crate::model::{ModelBinding, ModelBindings, ModelId, ModelInvocation, ModelNeedOpts};
    use mlua::Value;
    use mlua::{Lua, MultiValue};

    fn test_binding(alias: &str, dialect: ToolDialectId) -> ModelBinding {
        ModelBinding::new(
            alias,
            "a test capability",
            ModelId::from_validated("gateway", "m1"),
            ModelInvocation::from(&ModelNeedOpts::default()),
        )
        .with_dialect(dialect)
        .with_context(8192)
    }

    #[test]
    fn temperature_accepts_finite_in_domain_and_rejects_the_rest() {
        for good in [0.0, 0.7, 1.0, 2.0] {
            let got = value_as_temperature(&Value::Number(good)).expect("in-domain temperature");
            assert!(
                (got - good).abs() <= f64::EPSILON,
                "temperature {good} must pass through unchanged, got {got}"
            );
        }
        for bad in [-0.1, 2.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                value_as_temperature(&Value::Number(bad)).is_err(),
                "temperature {bad} must be rejected"
            );
        }
    }

    #[test]
    fn integer_and_number_temperatures_share_one_decode_and_domain_check() {
        // The Lua integer form is decoded through the same path as the number
        // form (no separate i32 gate) and validated by the same domain check.
        let from_integer =
            value_as_temperature(&Value::Integer(1)).expect("integer 1 is in-domain");
        let from_number =
            value_as_temperature(&Value::Number(1.0)).expect("number 1.0 is in-domain");
        assert!((from_integer - from_number).abs() <= f64::EPSILON);
        assert!(
            value_as_temperature(&Value::Integer(5)).is_err(),
            "an out-of-domain integer temperature must be rejected by the one domain check"
        );
    }

    #[test]
    fn dialect_getter_returns_the_closed_dialect_id() {
        // PF-LM-011: the Rust getter returns the closed identity type, not a
        // freshly allocated String.
        let handle = LuaModelHandle::from_binding(&test_binding("a", ToolDialectId::OpenAi));
        assert_eq!(handle.dialect(), ToolDialectId::OpenAi);
    }

    #[test]
    fn always_multi_arg_rolls_back_when_already_selected() {
        // PF-LM-003: a second multi-arg `models.always` must be rejected WITHOUT
        // leaving a half-recorded binding behind.
        let resolver = |_: &str, _: &ModelNeedOpts| {
            Ok(crate::model::ResolvedModel {
                id: ModelId::from_validated("gateway", "m1"),
                invocation: ModelInvocation::from(&ModelNeedOpts::default()),
                tool_dialect: ToolDialectId::OpenAi,
                context: 8192,
            })
        };
        let mut state = ModelBindingState::default();
        record_always_binding(
            &mut state,
            &resolver,
            "a",
            "desc",
            &ModelNeedOpts::default(),
        )
        .expect("the first models.always must succeed");
        assert_eq!(state.bindings.len(), 1);
        assert_eq!(state.always.as_deref(), Some("a"));

        let err = record_always_binding(
            &mut state,
            &resolver,
            "b",
            "desc",
            &ModelNeedOpts::default(),
        )
        .expect_err("a second models.always must be rejected");
        assert!(
            err.to_string().contains("at most once"),
            "error must explain the at-most-once rule: {err}"
        );
        assert_eq!(
            state.bindings.len(),
            1,
            "a rejected second models.always must not record a binding (rollback)"
        );
        assert_eq!(state.always.as_deref(), Some("a"));
    }

    #[test]
    fn close_model_scope_validates_binding_before_transition() {
        // PF-LM-007: when the selected alias has no frozen binding, close must
        // fail while the scope is still H2, never leaving a Closed scope with a
        // dangling selection.
        let bindings = ModelBindings::default();
        let runtime = Arc::new(Mutex::new(ModelRuntime::new()));
        runtime.lock().expect("lock the runtime").used = Some("ghost".to_owned());

        let err = close_model_scope_inner(&bindings, &runtime)
            .expect_err("a selected alias with no binding must fail the close");
        assert!(
            err.to_string().contains("no frozen binding"),
            "error must name the missing binding: {err}"
        );
        assert_eq!(
            runtime.lock().expect("lock the runtime").phase,
            ModelPhase::H2,
            "a failed close must not transition the scope to Closed"
        );
    }

    #[test]
    fn infer_options_absent_or_nil_are_accepted() {
        assert!(reject_infer_options(None).is_ok());
        assert!(reject_infer_options(Some(&Value::Nil)).is_ok());
    }

    #[test]
    fn infer_options_reject_a_table_or_any_non_nil_value() {
        let boolean = Value::Boolean(true);
        let integer = Value::Integer(1);
        for value in [&boolean, &integer] {
            let error = reject_infer_options(Some(value))
                .expect_err("a non-nil infer options argument must be rejected");
            assert!(
                error
                    .to_string()
                    .contains("does not accept a second argument"),
                "error must explain the rejection, got: {error}"
            );
        }
    }

    // PF-LM-014: direct coverage of every parser branch and state transition.

    fn lua_string(lua: &Lua, value: &str) -> Value {
        Value::String(lua.create_string(value).expect("create Lua string"))
    }

    #[test]
    fn parse_need_args_covers_each_branch() {
        let lua = Lua::new();
        // Missing description.
        let one: MultiValue = [lua_string(&lua, "writer")].into_iter().collect();
        assert!(parse_need_args(one).is_err(), "one argument is rejected");
        // Non-string alias.
        let bad_alias: MultiValue = [Value::Integer(1), lua_string(&lua, "desc")]
            .into_iter()
            .collect();
        assert!(
            parse_need_args(bad_alias).is_err(),
            "non-string alias fails"
        );
        // opts not a table.
        let bad_opts: MultiValue = [
            lua_string(&lua, "writer"),
            lua_string(&lua, "desc"),
            Value::Integer(3),
        ]
        .into_iter()
        .collect();
        assert!(parse_need_args(bad_opts).is_err(), "non-table opts fails");
        // Too many arguments.
        let too_many: MultiValue = [
            lua_string(&lua, "writer"),
            lua_string(&lua, "desc"),
            Value::Nil,
            Value::Nil,
        ]
        .into_iter()
        .collect();
        assert!(parse_need_args(too_many).is_err(), "four arguments fail");
        // Valid two-argument form.
        let ok: MultiValue = [lua_string(&lua, "writer"), lua_string(&lua, "desc")]
            .into_iter()
            .collect();
        let (alias, description, opts) = parse_need_args(ok).expect("valid need args");
        assert_eq!(alias, "writer");
        assert_eq!(description, "desc");
        assert_eq!(opts.temperature, None);
    }

    #[test]
    fn parse_opts_table_covers_each_key_and_rejects_unknown() {
        let lua = Lua::new();
        let table = lua.create_table().expect("table");
        table.set("thinking", true).expect("set thinking");
        table.set("context", 8192).expect("set context");
        table.set("temperature", 0.5).expect("set temperature");
        table.set("max_tokens", 256).expect("set max_tokens");
        let opts = parse_opts_table(&table).expect("all known keys parse");
        assert_eq!(opts.thinking, Some(true));
        assert_eq!(opts.context, Some(8192));
        assert_eq!(opts.temperature, Some(0.5));
        assert_eq!(opts.max_tokens, Some(256));

        let unknown = lua.create_table().expect("table");
        unknown.set("bogus", 1).expect("set bogus");
        assert!(
            parse_opts_table(&unknown).is_err(),
            "an unknown opts key must be rejected"
        );

        let non_string_key = lua.create_table().expect("table");
        non_string_key.set(1, "x").expect("set numeric key");
        assert!(
            parse_opts_table(&non_string_key).is_err(),
            "a non-string opts key must be rejected"
        );
    }

    #[test]
    fn scalar_decoders_cover_valid_and_invalid_inputs() {
        assert!(value_as_bool(&Value::Boolean(false), "thinking").is_ok());
        assert!(value_as_bool(&Value::Integer(1), "thinking").is_err());

        assert_eq!(value_as_u32(&Value::Integer(7), "context").expect("ok"), 7);
        assert_eq!(
            value_as_u32(&Value::Number(9.0), "context").expect("whole number ok"),
            9
        );
        assert!(value_as_u32(&Value::Integer(-1), "context").is_err());
        assert!(value_as_u32(&Value::Number(1.5), "context").is_err());
        assert!(value_as_u32(&Value::Boolean(true), "context").is_err());

        assert!(
            (decode_lua_number(&Value::Integer(2), "t").expect("int") - 2.0).abs() < f64::EPSILON
        );
        assert!(decode_lua_number(&Value::Boolean(true), "t").is_err());
    }

    #[test]
    fn parse_single_alias_and_validate_alias_branches() {
        let lua = Lua::new();
        let ok: MultiValue = [lua_string(&lua, "writer")].into_iter().collect();
        assert_eq!(
            parse_single_alias(&ok, "models.always").expect("string alias"),
            "writer"
        );
        let bad: MultiValue = [Value::Integer(1)].into_iter().collect();
        assert!(
            parse_single_alias(&bad, "models.always").is_err(),
            "a non-string alias must be rejected"
        );

        assert!(validate_alias("Writer_1-x").is_ok());
        assert!(validate_alias("").is_err(), "empty alias rejected");
        assert!(validate_alias("1abc").is_err(), "leading digit rejected");
        assert!(validate_alias("a b").is_err(), "space rejected");
    }

    #[test]
    fn model_runtime_transitions_h2_then_closed() {
        // State transition: a fresh runtime is H2, and closing an empty scope
        // yields no binding and transitions to Closed exactly once.
        let bindings = ModelBindings::default();
        let runtime = Arc::new(Mutex::new(ModelRuntime::new()));
        assert_eq!(runtime.lock().expect("lock").phase, ModelPhase::H2);
        let selected = close_model_scope_inner(&bindings, &runtime).expect("empty close is ok");
        assert!(selected.is_none(), "no use/always yields no binding");
        assert_eq!(runtime.lock().expect("lock").phase, ModelPhase::Closed);
        // A second close is rejected (can only close once).
        assert!(
            close_model_scope_inner(&bindings, &runtime).is_err(),
            "closing a Closed scope must fail"
        );
    }
}
