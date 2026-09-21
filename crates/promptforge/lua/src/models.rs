//! The Lua `models` host table: `use` / `default` / `get` / `infer`.
//!
//! Binding is frontmatter: the run's roles arrive pre-filled from prepare in
//! the shared [`ModelSet`], and the table selects among them by label.
//! `models.use` records the section's selection, `models.default` parks the
//! prompt-wide default, `models.get` inspects a bound role without selecting
//! it, and `models.infer` runs the one tool-free round through the
//! executor-installed hook.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::Mutex;

use mlua::{Lua, Table};

use promptforge_model_client::model::{ModelBinding, ModelId, ModelInvocation, ModelSet};

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

/// Builds the Agent-window hack's binding: an undeclared `models.get`
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

/// Section model-selection state: wraps the current `models.use` selection.
#[derive(Debug)]
pub struct ModelRuntime {
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

    /// Records a `models.use` selection, replacing any prior one: the
    /// selection is read at call time, so the latest call steers the next
    /// model round.
    pub(crate) fn select(&mut self, alias: String) {
        self.used = Some(alias);
    }
}

/// Installs the `models` table into one section VM (H1 included: there is
/// one install path for every section).
///
/// The table reads and writes the run's shared [`ModelSet`]: `models.use`
/// records the section's own selection in `runtime`, while
/// `models.default(label)` parks the prompt-wide default in the shared set -
/// a static prompt-wide fact, conventionally called from H1 but not
/// privileged to it. Re-selecting the same label is a no-op, so a shared
/// library replayed into every section may name the default; naming a
/// different label errors. There is no `models.bind`: binding is the
/// frontmatter's, and an unknown label is a hard error.
///
/// `raw_ids` is the Agent-window model-picker hack: when set, `models.get`
/// resolves an undeclared alias as a raw gateway catalog model id, so the
/// Workshop chat prompt can run `models.get(ui().selected_model)` without
/// declaring its model. Unset, an undeclared alias is the usual error.
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
        .create_function(move |_, label: String| -> mlua::Result<LuaModelHandle> {
            validate_alias(&label).map_err(mlua::Error::external)?;
            let binding = lock_models(&frozen)?
                .binding(&label)
                .cloned()
                .ok_or_else(|| {
                    mlua::Error::external(format!(
                        "models.use label {label:?} is not a bound model role"
                    ))
                })?;
            let mut state = state
                .lock()
                .map_err(|_| mlua::Error::external("model declaration runtime was poisoned"))?;
            state.select(label);
            Ok(LuaModelHandle::from_binding(&binding))
        })
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
            // The Agent-window hack: with the host's raw-id opt-in, an
            // undeclared alias resolves as a raw gateway catalog model id
            // under the fallback context window. The alias grammar does not
            // apply - gateway ids carry `/`, `.`, and `:` - so the id's own
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
