//! Model value types: validated temperature, bind/invocation options,
//! prompt-local bindings, and completion options.

use std::num::NonZeroU32;
use std::sync::Mutex;

use promptforge_types::models::ModelId;

/// The largest sampling temperature the backend accepts.
const TEMPERATURE_MAX: f64 = 2.0;

/// A validated sampling temperature: finite and within `[0.0, 2.0]`.
///
/// A temperature enters a request only as a `Temperature`, so the backend
/// receives only valid temperatures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Temperature(f64);

impl Temperature {
    /// Builds a temperature from a finite `value` within `[0.0, 2.0]`.
    ///
    /// # Errors
    /// Returns [`TemperatureError`] when `value` is `NaN` or an infinity, or
    /// falls outside `[0.0, 2.0]`.
    pub fn new(value: f64) -> std::result::Result<Temperature, TemperatureError> {
        if !value.is_finite() {
            return Err(TemperatureError::NotFinite);
        }
        if !(0.0..=TEMPERATURE_MAX).contains(&value) {
            return Err(TemperatureError::OutOfRange { value });
        }
        Ok(Temperature(value))
    }

    /// Returns the validated value.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Temperature {
    type Error = TemperatureError;

    fn try_from(value: f64) -> std::result::Result<Temperature, TemperatureError> {
        Temperature::new(value)
    }
}

/// The reason a sampling temperature was rejected.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum TemperatureError {
    /// The temperature was `NaN` or an infinity.
    #[error("temperature must be finite")]
    NotFinite,
    /// The temperature fell outside the supported `[0.0, 2.0]` range.
    #[error("temperature {value} is outside the supported range [0.0, 2.0]")]
    #[non_exhaustive]
    OutOfRange {
        /// The rejected value.
        value: f64,
    },
}

/// The fixed request settings that a model binding applies to every completion.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInvocation {
    /// The sampling temperature, when the binding's builder or a `models.use`
    /// option set one.
    ///
    /// It is a validated [`Temperature`], so the binding and the request carry
    /// only finite values within `[0.0, 2.0]`.
    pub temperature: Option<Temperature>,
    /// The maximum number of tokens to generate, when the binding's builder or
    /// a `models.use` option set a cap.
    ///
    /// The cap is at least one, so it always allows some output.
    /// `models.use` rejects a zero cap when it is called.
    pub max_tokens: Option<NonZeroU32>,
    /// The thinking switch, sent as `chat_template_kwargs.enable_thinking`,
    /// when set.
    pub thinking: Option<bool>,
}

/// A prompt-local alias bound to one model and its fixed request settings.
// No `Eq`: the frozen invocation holds an `f64` temperature.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelBinding {
    alias: String,
    description: String,
    id: ModelId,
    invocation: ModelInvocation,
    context: NonZeroU32,
    /// The bound role's full keyword set (the closed frontmatter
    /// vocabulary, kebab-case), exposed on the Lua handle as
    /// `capabilities`. Empty for bindings built outside a role fill.
    capabilities: Vec<String>,
}

impl ModelBinding {
    /// Builds a binding from every part a resolved model requires.
    ///
    /// The `context` window is a required argument of at least one token, so a
    /// binding is always complete once built.
    #[must_use]
    pub fn new(
        alias: impl Into<String>,
        description: impl Into<String>,
        id: ModelId,
        invocation: ModelInvocation,
        context: NonZeroU32,
    ) -> Self {
        Self {
            alias: alias.into(),
            description: description.into(),
            id,
            invocation,
            context,
            capabilities: Vec::new(),
        }
    }

    /// Sets the bound role's capability keywords, which Lua code reads as the
    /// `capabilities` field of the model handle.
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Replaces the binding's request settings.
    #[must_use]
    pub fn with_invocation(mut self, invocation: ModelInvocation) -> Self {
        self.invocation = invocation;
        self
    }

    /// Returns the bound role's capability keywords.
    #[must_use]
    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    /// Returns the exact prompt-local alias.
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Returns the declared capability description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the stable identity of the bound model.
    #[must_use]
    pub fn id(&self) -> &ModelId {
        &self.id
    }

    /// Returns the binding's request settings.
    #[must_use]
    pub fn invocation(&self) -> &ModelInvocation {
        &self.invocation
    }

    /// Returns the catalog context window size in tokens (always at least one).
    #[must_use]
    pub fn context(&self) -> NonZeroU32 {
        self.context
    }

    /// Builds the [`CompletionOptions`] for a completion made under this
    /// binding, from the model's name and the binding's request settings.
    #[must_use]
    pub fn completion_options(&self) -> CompletionOptions {
        CompletionOptions {
            model: self.id.name().to_owned(),
            temperature: self.invocation.temperature,
            max_tokens: self.invocation.max_tokens,
            thinking: self.invocation.thinking,
        }
    }
}

/// Per-call settings merged into a chat-completions request body.
///
/// Build one with [`CompletionOptions::new`] and its `with_*` setters.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CompletionOptions {
    /// The caller-facing model name sent on the wire.
    pub(super) model: String,
    /// Sampling temperature (a validated [`Temperature`]).
    temperature: Option<Temperature>,
    /// Maximum generation tokens (always non-zero).
    max_tokens: Option<NonZeroU32>,
    /// When set, emits `chat_template_kwargs.enable_thinking`.
    thinking: Option<bool>,
}

// No `Eq`: `temperature` is an `Option<f64>`, so equality is not reflexive for
// NaN. A manual `impl Eq` here would claim a total equivalence the field cannot
// honor, breaking every `Eq`/`Hash` consumer's contract.

impl CompletionOptions {
    /// Builds options for `model`, with the temperature, token cap, and
    /// thinking switch set to `None`.
    #[must_use]
    pub fn new(model: impl Into<String>) -> CompletionOptions {
        CompletionOptions {
            model: model.into(),
            temperature: None,
            max_tokens: None,
            thinking: None,
        }
    }

    /// Sets the sampling temperature after validating it is finite and within
    /// the backend-supported range `[0.0, 2.0]`.
    ///
    /// # Errors
    /// Returns [`TemperatureError`] when `temperature` is `NaN` or an infinity,
    /// or falls outside `[0.0, 2.0]`, so only a valid temperature is sent.
    pub fn with_temperature(
        mut self,
        temperature: f64,
    ) -> std::result::Result<CompletionOptions, TemperatureError> {
        self.temperature = Some(Temperature::new(temperature)?);
        Ok(self)
    }

    /// Sets the maximum number of tokens to generate.
    ///
    /// The cap is a [`NonZeroU32`], so it always allows some output.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: NonZeroU32) -> CompletionOptions {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Sets the thinking switch, sent as `chat_template_kwargs.enable_thinking`.
    #[must_use]
    pub fn with_thinking(mut self, thinking: bool) -> CompletionOptions {
        self.thinking = Some(thinking);
        self
    }

    /// Replaces the model name sent in the request and keeps every other
    /// setting.
    ///
    /// When the caller serves a request with a substitute for the bound model,
    /// it sets the substitute's name here so the request names it.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> CompletionOptions {
        self.model = model.into();
        self
    }

    /// Returns the caller-facing model name sent in the request.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the sampling temperature, when one was set.
    #[must_use]
    pub fn temperature(&self) -> Option<Temperature> {
        self.temperature
    }

    /// Returns the maximum number of tokens to generate, when a cap was set.
    #[must_use]
    pub fn max_tokens(&self) -> Option<NonZeroU32> {
        self.max_tokens
    }

    /// Returns the `enable_thinking` switch, when it was set.
    #[must_use]
    pub fn thinking(&self) -> Option<bool> {
        self.thinking
    }
}

/// The run's model set: the prompt-level bindings produced by live H1
/// execution plus the prompt-wide `default` alias.
// No `Eq`: bindings hold `f64` temperatures transitively.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelSet {
    /// The bindings in declaration order.
    pub bindings: Vec<ModelBinding>,
    /// The prompt-wide default alias set by `models.default`, if any. No
    /// inherent `default()` accessor: it would shadow `Default::default()`
    /// at every construction site; readers use the field or the
    /// [`ModelView`] trait.
    pub default: Option<String>,
}

impl ModelSet {
    /// Reassembles a set from owned snapshots of its two parts (the
    /// [`ModelView`] read pair).
    #[must_use]
    pub fn from_parts(bindings: Vec<ModelBinding>, default: Option<String>) -> Self {
        Self { bindings, default }
    }

    /// Returns bindings in declaration order.
    #[must_use]
    pub fn bindings(&self) -> &[ModelBinding] {
        &self.bindings
    }

    /// Returns the binding for `alias`, if it was declared.
    #[must_use]
    pub fn binding(&self, alias: &str) -> Option<&ModelBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }
}

/// The run's model-set lock was poisoned.
///
/// This is the run's own mutex failing, not a model failure, so it has no
/// [`CompletionErrorKind`](crate::model::CompletionErrorKind) and no retry
/// advice. The Engine and Lua crates map it to their Lua error, and the
/// facade does not re-export it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("model set mutex was poisoned")]
#[non_exhaustive]
pub struct ModelSetError;

/// The read-only view over the run's [`ModelSet`].
///
/// The run context shares the set as `Arc<dyn ModelView>`; the live H1 pass
/// writes through its own concrete `Arc<Mutex<ModelSet>>` handle, and once
/// that VM is dropped no write handle remains. The trait is read-only, so
/// post-H1 frozenness is structural. Every method locks briefly and returns
/// an owned snapshot: a mutex guard cannot outlive the call.
pub trait ModelView: Send + Sync {
    /// Returns an owned snapshot of the bindings in declaration order.
    ///
    /// # Errors
    /// Returns [`ModelSetError`] if the set's mutex is poisoned.
    fn bindings(&self) -> Result<Vec<ModelBinding>, ModelSetError>;

    /// Returns the prompt-wide default alias set by `models.default`, if any.
    ///
    /// # Errors
    /// Returns [`ModelSetError`] if the set's mutex is poisoned.
    fn default(&self) -> Result<Option<String>, ModelSetError>;

    /// Returns an owned clone of the binding for `alias`, if it was
    /// declared.
    ///
    /// # Errors
    /// Returns [`ModelSetError`] if the set's mutex is poisoned.
    fn binding(&self, alias: &str) -> Result<Option<ModelBinding>, ModelSetError>;
}

/// Maps a poisoned set lock to [`ModelSetError`].
fn lock_model_set(
    set: &Mutex<ModelSet>,
) -> Result<std::sync::MutexGuard<'_, ModelSet>, ModelSetError> {
    set.lock().map_err(|_| ModelSetError)
}

impl ModelView for Mutex<ModelSet> {
    fn bindings(&self) -> Result<Vec<ModelBinding>, ModelSetError> {
        Ok(lock_model_set(self)?.bindings.clone())
    }

    fn default(&self) -> Result<Option<String>, ModelSetError> {
        Ok(lock_model_set(self)?.default.clone())
    }

    fn binding(&self, alias: &str) -> Result<Option<ModelBinding>, ModelSetError> {
        Ok(lock_model_set(self)?.binding(alias).cloned())
    }
}

#[cfg(test)]
#[path = "options-tests.rs"]
mod tests;
