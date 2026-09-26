//! The Lua `models` host table: `use` / `default` / `get` / `infer`.
//!
//! Binding is frontmatter: the run's roles arrive pre-filled from prepare in
//! the shared [`ModelSet`], and the table selects among them by label.
//! `models.use` records the section's selection with its optional sampling
//! options, `models.default` parks the prompt-wide default, `models.get`
//! inspects a bound role without selecting it, and `models.infer` runs the
//! one tool-free round through the executor-installed hook.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::Mutex;

use mlua::{Lua, MultiValue, Table, Value};

use promptforge_model_client::model::{
    ModelBinding, ModelId, ModelInvocation, ModelSet, Temperature, TemperatureError,
};

use crate::alias::validate_alias;
use crate::{Error, Result};

#[path = "models-userdata.rs"]
mod userdata;

pub(crate) use userdata::{LuaModelHandle, ModelsInferHook};

/// The context window a raw gateway-id binding records: catalog metadata
/// the hack never sees, so a conservative default keeps the compactor
/// precheck safe rather than refusing the model. Mirrors the Workshop's
/// own catalog fallback.
const RAW_ID_CONTEXT: NonZeroU32 = match NonZeroU32::new(8192) {
    Some(value) => value,
    None => unreachable!(),
};

/// Builds the raw-model-id fallback's binding: an undeclared `models.get`
/// alias resolved as a raw gateway catalog model id, with no invocation
/// overrides and the fallback context window.
fn raw_gateway_binding(alias: &str) -> mlua::Result<ModelBinding> {
    let id = ModelId::gateway(alias).map_err(|error| {
        mlua::Error::external(format!("models.get model id {alias:?} is invalid: {error}"))
    })?;
    Ok(ModelBinding::new(
        alias,
        alias,
        id,
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        RAW_ID_CONTEXT,
    ))
}

/// Dispatches a `models.infer(prompt)` call through the executor-installed
/// [`ModelsInferHook`] app data.
///
/// The hook owns everything else (current-model resolution, gateway
/// client, section identity). The call runs the one infer shape: a single
/// tool-free round on a fresh conversation that never sets `reply` or
/// touches `sys`.
fn call_models_infer_hook(lua: &Lua, prompt: &str) -> mlua::Result<String> {
    let hook = lua
        .app_data_ref::<ModelsInferHook>()
        .ok_or_else(|| {
            mlua::Error::external("models.infer is not available outside section execution")
        })?
        .clone();
    hook(lua, prompt)
}

/// Locks the run's shared model set, mapping a poisoned lock to the Lua
/// boundary error every host callback uses.
fn lock_models(set: &Mutex<ModelSet>) -> mlua::Result<std::sync::MutexGuard<'_, ModelSet>> {
    set.lock()
        .map_err(|_| mlua::Error::external("model set mutex was poisoned"))
}

/// The sampling options a `models.use` call sets on its selection. An
/// omitted field keeps the role's own value.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct UseOptions {
    temperature: Option<Temperature>,
    max_tokens: Option<NonZeroU32>,
}

impl UseOptions {
    /// Overrides the binding's invocation with the fields these options set.
    pub(crate) fn apply(self, binding: ModelBinding) -> ModelBinding {
        let mut invocation = binding.invocation().clone();
        invocation.temperature = self.temperature.or(invocation.temperature);
        invocation.max_tokens = self.max_tokens.or(invocation.max_tokens);
        binding.with_invocation(invocation)
    }
}

/// A `models.use` option rejection stating required versus actual.
fn invalid_option(option: &str, required: &str, actual: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::external(format!(
        "models.use option {option} must be {required}, got {actual}"
    ))
}

/// Decodes `temperature`: a Lua integer or number, bounded only by
/// [`Temperature::new`].
fn decode_temperature(value: &Value) -> mlua::Result<Temperature> {
    let number = match value {
        Value::Number(number) => Ok(*number),
        Value::Integer(number) =>
        {
            #[expect(
                clippy::cast_precision_loss,
                reason = "any magnitude that loses precision fails the [0.0, 2.0] check"
            )]
            Ok(*number as f64)
        }
        other => Err(invalid_option("temperature", "a number", other.type_name())),
    }?;
    Temperature::new(number).map_err(|error| match error {
        TemperatureError::NotFinite => invalid_option("temperature", "finite", number),
        other => mlua::Error::external(format!("models.use option {other}")),
    })
}

/// Decodes `max_tokens`: a positive integer that fits a [`NonZeroU32`],
/// given as a Lua integer or an integral float.
fn decode_max_tokens(value: &Value) -> mlua::Result<NonZeroU32> {
    let count = match value {
        Value::Integer(number) => u32::try_from(*number).ok(),
        Value::Number(number)
            if number.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(number) =>
        {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "integral and range checked against u32"
            )]
            Some(*number as u32)
        }
        _ => None,
    };
    count.and_then(NonZeroU32::new).ok_or_else(|| {
        let actual = match value {
            Value::Integer(number) => number.to_string(),
            Value::Number(number) => number.to_string(),
            other => other.type_name().to_owned(),
        };
        invalid_option("max_tokens", "an integer in [1, 4294967295]", actual)
    })
}

/// Where `models.use` checks one option entry: non-string keys first,
/// grouped by type because their rejection names only the type, then names
/// bytewise. `pairs` walks in the state's hash-seed order, so checking in
/// this order makes the first rejection a function of the table's contents.
fn option_check_order(key: &Value) -> (bool, Vec<u8>) {
    match key {
        Value::String(name) => (true, name.as_bytes().to_vec()),
        other => (false, other.type_name().as_bytes().to_vec()),
    }
}

/// Decodes the `models.use` arguments after the label: an optional options
/// table and nothing more.
fn parse_use_options(options: Value, rest: &MultiValue) -> mlua::Result<UseOptions> {
    if !rest.is_empty() {
        return Err(mlua::Error::external(format!(
            "models.use takes at most 2 arguments, got {}",
            rest.len() + 2
        )));
    }
    let table = match options {
        Value::Nil => return Ok(UseOptions::default()),
        Value::Table(table) => table,
        other => {
            return Err(mlua::Error::external(format!(
                "models.use options must be a table, got {}",
                other.type_name()
            )));
        }
    };
    let mut entries = table
        .pairs::<Value, Value>()
        .collect::<mlua::Result<Vec<_>>>()?;
    entries.sort_by_cached_key(|(key, _)| option_check_order(key));
    let mut parsed = UseOptions::default();
    for (key, value) in entries {
        let Value::String(key) = key else {
            return Err(mlua::Error::external(format!(
                "models.use option names must be strings, got {}",
                key.type_name()
            )));
        };
        match key.to_string_lossy().as_str() {
            "temperature" => parsed.temperature = Some(decode_temperature(&value)?),
            "max_tokens" => parsed.max_tokens = Some(decode_max_tokens(&value)?),
            other => {
                return Err(mlua::Error::external(format!(
                    "models.use option {other:?} is unknown: expected temperature or max_tokens"
                )));
            }
        }
    }
    Ok(parsed)
}

/// Section model-selection state: the current `models.use` label and the
/// options it set.
#[derive(Debug)]
pub struct ModelRuntime {
    used: Option<(String, UseOptions)>,
}

impl ModelRuntime {
    pub(crate) fn new() -> Self {
        ModelRuntime { used: None }
    }

    /// The current `models.use` label and its options, if any.
    pub(crate) fn selection(&self) -> Option<(&str, UseOptions)> {
        self.used
            .as_ref()
            .map(|(label, options)| (label.as_str(), *options))
    }

    /// Records a `models.use` selection, replacing any prior label and
    /// options: the selection is read at call time, so the latest call
    /// steers the next model round.
    pub(crate) fn select(&mut self, alias: String, options: UseOptions) {
        self.used = Some((alias, options));
    }
}

/// Installs the `models` table into one section VM (H1 included: there is
/// one install path for every section).
///
/// The table reads and writes the run's shared [`ModelSet`]:
/// `models.use(label, options?)` records the section's own selection in
/// `runtime`, with an optional `temperature` / `max_tokens` table that
/// applies to rounds on that selection and to the handle it returns, while
/// `models.default(label)` records the prompt-wide default in the shared
/// set - a static prompt-wide fact, conventionally called from H1 but not
/// privileged to it. Re-selecting the same label is a no-op, so a shared
/// library replayed into every section may name the default; naming a
/// different label errors. There is no `models.bind`: binding is the
/// frontmatter's, and an unknown label is a hard error.
///
/// `raw_ids` is the raw-model-id fallback, on whenever the host passes a
/// host-state snapshot (`RunContext::ui`): when set, `models.get` resolves
/// an undeclared alias as a raw gateway catalog model id, so the built-in
/// chat prompt can run `models.get(ui().selected_model)` without declaring
/// its model. Unset, an undeclared alias is the usual error.
///
/// The coroutine shim layer installs the suspending `models.loop`, because
/// yield cannot cross the Rust callback boundary.
///
/// # Errors
/// Returns [`Error::Lua`] if a Lua table or callback cannot be created or
/// installed.
pub(crate) fn install_models(
    lua: &Lua,
    globals: &Table,
    set: &Arc<Mutex<ModelSet>>,
    runtime: &Arc<Mutex<ModelRuntime>>,
    raw_ids: bool,
) -> Result<()> {
    let models = lua.create_table().map_err(Error::lua)?;

    let frozen = Arc::clone(set);
    let state = Arc::clone(runtime);
    let use_fn = lua
        .create_function(
            move |_,
                  (label, options, rest): (String, Value, MultiValue)|
                  -> mlua::Result<LuaModelHandle> {
                validate_alias(&label).map_err(mlua::Error::external)?;
                let binding = lock_models(&frozen)?
                    .binding(&label)
                    .cloned()
                    .ok_or_else(|| {
                        mlua::Error::external(format!(
                            "models.use label {label:?} is not a bound model role"
                        ))
                    })?;
                let options = parse_use_options(options, &rest)?;
                let mut state = state
                    .lock()
                    .map_err(|_| mlua::Error::external("model declaration runtime was poisoned"))?;
                state.select(label, options);
                Ok(LuaModelHandle::from_binding(&options.apply(binding)))
            },
        )
        .map_err(Error::lua)?;
    models.set("use", use_fn).map_err(Error::lua)?;

    let frozen = Arc::clone(set);
    let default_fn = lua
        .create_function(move |_, label: String| -> mlua::Result<LuaModelHandle> {
            validate_alias(&label).map_err(mlua::Error::external)?;
            let mut set = lock_models(&frozen)?;
            let binding = set
                .binding(&label)
                .cloned()
                .ok_or_else(|| {
                    mlua::Error::external(format!(
                        "models.default label {label:?} is not a bound model role"
                    ))
                })?;
            match &set.default {
                // Idempotent under the shared-library replay: every
                // section re-runs the library, so naming the same default
                // again is a no-op.
                Some(existing) if existing == &label => {}
                Some(existing) => {
                    return Err(mlua::Error::external(format!(
                        "models.default is already {existing:?}: the prompt-wide default cannot change mid-run"
                    )));
                }
                None => set.default = Some(label),
            }
            Ok(LuaModelHandle::from_binding(&binding))
        })
        .map_err(Error::lua)?;
    models.set("default", default_fn).map_err(Error::lua)?;

    let frozen = Arc::clone(set);
    let get_fn = lua
        .create_function(move |_, alias: String| -> mlua::Result<LuaModelHandle> {
            if let Some(binding) = lock_models(&frozen)?.binding(&alias).cloned() {
                return Ok(LuaModelHandle::from_binding(&binding));
            }
            // The raw-model-id fallback: with the host's raw-id opt-in, an
            // undeclared alias resolves as a raw gateway catalog model id
            // under the fallback context window. The alias grammar does not
            // apply - gateway ids include `/`, `.`, and `:` - so the id's own
            // validation is the only gate.
            if raw_ids {
                let binding = raw_gateway_binding(&alias)?;
                return Ok(LuaModelHandle::from_binding(&binding));
            }
            validate_alias(&alias).map_err(mlua::Error::external)?;
            Err(mlua::Error::external(format!(
                "models.get alias {alias:?} is not a bound model role"
            )))
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
#[path = "models-tests.rs"]
mod tests;
