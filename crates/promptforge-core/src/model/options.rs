//! Model value types: validated temperature, thinking mode, descriptor,
//! bind/invocation options, prompt-local bindings, and completion options.

use std::num::NonZeroU32;

use serde::Deserialize;

use super::ModelId;
use crate::dialects::{ToolDialectId, ToolsMode};

/// The largest sampling temperature the backend accepts.
const TEMPERATURE_MAX: f64 = 2.0;

/// A validated sampling temperature: finite and within `[0.0, 2.0]`.
///
/// Building a [`Temperature`] is the only in-crate way to place a temperature
/// into a request, so a `NaN`, an infinity, or an out-of-range value is
/// unrepresentable rather than serialized into a backend-invalid request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Temperature(f64);

impl Temperature {
    /// Builds a temperature, rejecting non-finite and out-of-range values.
    ///
    /// # Errors
    /// Returns [`TemperatureError`] when `value` is not finite or falls outside
    /// `[0.0, 2.0]`.
    pub(crate) fn new(value: f64) -> std::result::Result<Temperature, TemperatureError> {
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
    pub(crate) fn get(self) -> f64 {
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

/// Whether a catalogued model can emit thinking tokens.
///
/// # Examples
///
/// ```
/// use promptforge_core::model::ThinkingMode;
///
/// // Deserialized from the lowercase gateway wire form.
/// let mode: ThinkingMode = serde_json::from_str("\"switchable\"")?;
/// assert_eq!(mode, ThinkingMode::Switchable);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ThinkingMode {
    /// The backend never emits thinking tokens.
    Never,
    /// The backend always emits thinking tokens.
    Always,
    /// The client may turn thinking on or off per request.
    Switchable,
}

/// One catalogued model with live-resolution metadata.
///
/// `#[non_exhaustive]` so the descriptor is only ever built through
/// [`ModelDescriptor::new`] and its validated context window is preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelDescriptor {
    id: ModelId,
    description: String,
    context: NonZeroU32,
    thinking: ThinkingMode,
    tool_dialect: ToolDialectId,
}

impl ModelDescriptor {
    /// Builds a descriptor from its identity and catalog fields.
    ///
    /// The context window is a [`NonZeroU32`], so a zero-token window is
    /// unrepresentable. Defaults `tool_dialect` to [`ToolDialectId::OpenAi`].
    /// Use [`Self::with_dialect`] to override; the tools mode is always derived
    /// from the dialect, never stored independently.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU32;
    /// use promptforge_core::model::{ModelDescriptor, ModelId, ThinkingMode};
    ///
    /// let context = NonZeroU32::new(131_072).ok_or("context is non-zero")?;
    /// let model = ModelDescriptor::new(
    ///     ModelId::gateway("analyst")?,
    ///     "A careful analysis model",
    ///     context,
    ///     ThinkingMode::Switchable,
    /// );
    /// assert_eq!(model.context(), context);
    /// assert_eq!(model.thinking(), ThinkingMode::Switchable);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(
        id: ModelId,
        description: impl Into<String>,
        context: NonZeroU32,
        thinking: ThinkingMode,
    ) -> Self {
        Self {
            id,
            description: description.into(),
            context,
            thinking,
            tool_dialect: ToolDialectId::OpenAi,
        }
    }

    /// Sets the tool dialect. The tools mode is derived from it on demand.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU32;
    /// use promptforge_core::model::{ModelDescriptor, ModelId, ThinkingMode};
    /// use promptforge_core::dialects::{ToolDialectId, ToolsMode};
    ///
    /// let model = ModelDescriptor::new(
    ///     ModelId::gateway("gemma-local")?,
    ///     "A local gemma model",
    ///     NonZeroU32::new(32_768).ok_or("context is non-zero")?,
    ///     ThinkingMode::Never,
    /// )
    /// .with_dialect(ToolDialectId::Gemma3ToolCode);
    /// assert_eq!(model.tool_dialect(), ToolDialectId::Gemma3ToolCode);
    /// assert_eq!(model.tools_mode(), ToolsMode::Emulated);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn with_dialect(mut self, dialect: ToolDialectId) -> Self {
        self.tool_dialect = dialect;
        self
    }

    /// Returns the stable identity.
    #[must_use]
    pub fn id(&self) -> &ModelId {
        &self.id
    }

    /// Returns the prose used for semantic resolve.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the context window size in tokens (always non-zero).
    #[must_use]
    pub fn context(&self) -> NonZeroU32 {
        self.context
    }

    /// Returns the thinking capability.
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }

    /// Returns the tool-calling dialect for this model.
    #[must_use]
    pub fn tool_dialect(&self) -> ToolDialectId {
        self.tool_dialect
    }

    /// Returns whether tool calls are native or emulated.
    ///
    /// Derived from [`Self::tool_dialect`]; the mode is never stored separately,
    /// so it cannot drift from the canonical dialect.
    #[must_use]
    pub fn tools_mode(&self) -> ToolsMode {
        self.tool_dialect.tools_mode()
    }
}

/// Optional hard constraints and invocation parameters from `models.bind`.
///
/// `context` and `thinking` filter the catalog. `temperature`, `max_tokens`,
/// and a requested `thinking` switch ride on each completion for the binding.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModelBindOpts {
    /// When set, filters models by thinking capability and freezes the switch.
    pub(crate) thinking: Option<bool>,
    /// Minimum context window size in tokens.
    ///
    /// A [`NonZeroU32`] (MODEL-003): a zero-token minimum is a nonsensical
    /// constraint and is unrepresentable, rejected at the parse boundary.
    pub(crate) context: Option<NonZeroU32>,
    /// Sampling temperature for every complete under this binding.
    ///
    /// A validated [`Temperature`] (PF-LM-004): a non-finite or out-of-range
    /// value is unrepresentable, so an invalid temperature can never reach the
    /// binding or the wire.
    pub(crate) temperature: Option<Temperature>,
    /// Maximum generation tokens for every complete under this binding.
    ///
    /// A [`NonZeroU32`] (MODEL-003): a zero-token generation cap would forbid
    /// all output, so it is unrepresentable and rejected at the parse boundary.
    pub(crate) max_tokens: Option<NonZeroU32>,
}

// No `Eq`: `temperature` is a `Temperature` (an `f64` newtype), so equality is
// not reflexive for a NaN placed in-crate via the private field.

/// Frozen per-request fields carried by a resolved model binding.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelInvocation {
    /// Sampling temperature, when the bind declared one.
    pub(crate) temperature: Option<Temperature>,
    /// Maximum generation tokens, when the bind declared one (always non-zero).
    pub(crate) max_tokens: Option<NonZeroU32>,
    /// Thinking switch for `chat_template_kwargs.enable_thinking`, when set.
    pub(crate) thinking: Option<bool>,
}

// No `Eq`: `temperature` is an `f64`, so equality is not reflexive for NaN.

impl From<&ModelBindOpts> for ModelInvocation {
    fn from(opts: &ModelBindOpts) -> Self {
        Self {
            temperature: opts.temperature,
            max_tokens: opts.max_tokens,
            thinking: opts.thinking,
        }
    }
}

/// One prompt-local alias bound to a model identity and frozen invocation.
// No `Eq`: the frozen invocation carries an `f64` temperature.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelBinding {
    alias: String,
    description: String,
    id: ModelId,
    invocation: ModelInvocation,
    tool_dialect: ToolDialectId,
    context: NonZeroU32,
}

impl ModelBinding {
    /// Builds a binding atomically from every part a resolved model requires.
    ///
    /// The `tool_dialect` and the non-zero `context` window are required
    /// arguments (MODEL-006): there is no fabricated OpenAI default and no
    /// zero-context sentinel patched in by a later setter, so a binding cannot
    /// exist in a half-initialized state.
    #[must_use]
    pub(crate) fn new(
        alias: impl Into<String>,
        description: impl Into<String>,
        id: ModelId,
        invocation: ModelInvocation,
        tool_dialect: ToolDialectId,
        context: NonZeroU32,
    ) -> Self {
        Self {
            alias: alias.into(),
            description: description.into(),
            id,
            invocation,
            tool_dialect,
            context,
        }
    }

    /// Returns the exact prompt-local alias.
    #[must_use]
    pub(crate) fn alias(&self) -> &str {
        &self.alias
    }

    /// Returns the declared capability description.
    #[must_use]
    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    /// Returns the selected stable identity.
    #[must_use]
    pub(crate) fn id(&self) -> &ModelId {
        &self.id
    }

    /// Returns the frozen per-request fields.
    #[must_use]
    pub(crate) fn invocation(&self) -> &ModelInvocation {
        &self.invocation
    }

    /// Returns the tool dialect for this binding.
    #[must_use]
    pub(crate) fn tool_dialect(&self) -> ToolDialectId {
        self.tool_dialect
    }

    /// Returns the catalog context window size in tokens (always non-zero).
    #[must_use]
    pub(crate) fn context(&self) -> NonZeroU32 {
        self.context
    }

    /// Builds [`CompletionOptions`] for every complete under this binding.
    #[must_use]
    pub(crate) fn completion_options(&self) -> CompletionOptions {
        CompletionOptions {
            model: self.id.name().to_owned(),
            temperature: self.invocation.temperature,
            max_tokens: self.invocation.max_tokens,
            thinking: self.invocation.thinking,
            tool_dialect: self.tool_dialect,
        }
    }
}

/// Per-call fields merged into a chat-completions request body.
///
/// Built through [`CompletionOptions::new`] and its `with_*` setters; the fields
/// are private so a caller cannot assemble an inconsistent request by hand.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CompletionOptions {
    /// The caller-facing model name sent on the wire.
    pub(crate) model: String,
    /// Sampling temperature (a validated [`Temperature`]).
    pub(crate) temperature: Option<Temperature>,
    /// Maximum generation tokens (always non-zero).
    pub(crate) max_tokens: Option<NonZeroU32>,
    /// When set, emits `chat_template_kwargs.enable_thinking`.
    pub(crate) thinking: Option<bool>,
    /// Which tool-calling dialect to use for this completion.
    pub(crate) tool_dialect: ToolDialectId,
}

// No `Eq`: `temperature` is an `Option<f64>`, so equality is not reflexive for
// NaN. A manual `impl Eq` here would claim a total equivalence the field cannot
// honor, breaking every `Eq`/`Hash` consumer's contract.

impl CompletionOptions {
    /// Builds options for `model` under the given tool-calling `dialect`, with no
    /// temperature, token cap, or thinking switch.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU32;
    /// use promptforge_core::model::CompletionOptions;
    /// use promptforge_core::dialects::ToolDialectId;
    ///
    /// let options = CompletionOptions::new("analyst", ToolDialectId::OpenAi)
    ///     .with_temperature(0.2)?
    ///     .with_max_tokens(NonZeroU32::new(256).ok_or("max tokens is non-zero")?)
    ///     .with_thinking(false);
    /// let _ = options;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(model: impl Into<String>, dialect: ToolDialectId) -> CompletionOptions {
        CompletionOptions {
            model: model.into(),
            temperature: None,
            max_tokens: None,
            thinking: None,
            tool_dialect: dialect,
        }
    }

    /// Sets the sampling temperature after validating it is finite and within
    /// the backend-supported range `[0.0, 2.0]`.
    ///
    /// # Errors
    /// Returns [`TemperatureError`] when `temperature` is not finite or falls
    /// outside `[0.0, 2.0]`, so an invalid temperature never reaches the wire.
    pub fn with_temperature(
        mut self,
        temperature: f64,
    ) -> std::result::Result<CompletionOptions, TemperatureError> {
        self.temperature = Some(Temperature::new(temperature)?);
        Ok(self)
    }

    /// Sets the maximum generation tokens.
    ///
    /// Takes a [`NonZeroU32`] (MODEL-003) so a zero generation cap, which would
    /// forbid all output, cannot be placed into a request.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: NonZeroU32) -> CompletionOptions {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Sets the `enable_thinking` switch.
    #[must_use]
    pub fn with_thinking(mut self, thinking: bool) -> CompletionOptions {
        self.thinking = Some(thinking);
        self
    }
}

/// Immutable prompt-level model bindings from live H1 execution.
// No `Eq`: bindings carry `f64` temperatures transitively.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModelBindings {
    bindings: Vec<ModelBinding>,
    default: Option<String>,
}

impl ModelBindings {
    /// Returns bindings in declaration order.
    #[must_use]
    pub(crate) fn bindings(&self) -> &[ModelBinding] {
        &self.bindings
    }

    /// Returns the prompt-wide default alias set by `models.default`, if any.
    #[must_use]
    pub(crate) fn default(&self) -> Option<&str> {
        self.default.as_deref()
    }

    /// Returns the binding for `alias`, if it was declared.
    pub(crate) fn binding(&self, alias: &str) -> Option<&ModelBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }

    /// Builds the immutable binding set from its ordered entries and default.
    pub(crate) fn from_parts(bindings: Vec<ModelBinding>, default: Option<String>) -> Self {
        Self { bindings, default }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_invocation_equality_is_not_reflexive_for_nan() {
        // Documents why these float-bearing types intentionally do not implement
        // `Eq`: a NaN temperature is not equal to itself. Built with the in-crate
        // private `Temperature(NaN)` tuple (colocated here so it can reach the
        // private field) to prove the soundness reason `Eq` is withheld.
        let nan = ModelInvocation {
            temperature: Some(Temperature(f64::NAN)),
            max_tokens: None,
            thinking: None,
        };
        assert_ne!(nan, nan.clone());
    }

    #[test]
    fn completion_options_equality_is_not_reflexive_for_nan() {
        // `CompletionOptions` carries an `Option<Temperature>` (an `f64` newtype)
        // temperature, so it must not implement `Eq`: a NaN temperature is not
        // equal to itself. This assertion documents the violated reflexivity
        // contract even though Rust permits a manual `Eq` implementation.
        let options = CompletionOptions {
            model: "m".to_owned(),
            temperature: Some(Temperature(f64::NAN)),
            max_tokens: None,
            thinking: None,
            tool_dialect: ToolDialectId::OpenAi,
        };
        assert_ne!(options, options.clone());
    }

    #[test]
    fn with_temperature_rejects_non_finite_and_out_of_range() {
        let base = || CompletionOptions::new("m", ToolDialectId::OpenAi);
        assert_eq!(
            base().with_temperature(f64::NAN),
            Err(TemperatureError::NotFinite)
        );
        assert_eq!(
            base().with_temperature(f64::INFINITY),
            Err(TemperatureError::NotFinite)
        );
        assert!(matches!(
            base().with_temperature(-0.1),
            Err(TemperatureError::OutOfRange { .. })
        ));
        assert!(matches!(
            base().with_temperature(2.5),
            Err(TemperatureError::OutOfRange { .. })
        ));
        // The range endpoints and an interior value are accepted.
        assert_eq!(
            base()
                .with_temperature(0.0)
                .expect("0.0 is valid")
                .temperature
                .map(Temperature::get),
            Some(0.0)
        );
        assert_eq!(
            base()
                .with_temperature(TEMPERATURE_MAX)
                .expect("2.0 is valid")
                .temperature
                .map(Temperature::get),
            Some(TEMPERATURE_MAX)
        );
        assert_eq!(
            base()
                .with_temperature(0.7)
                .expect("0.7 is valid")
                .temperature
                .map(Temperature::get),
            Some(0.7)
        );
    }
}
