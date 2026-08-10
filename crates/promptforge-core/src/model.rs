//! Prompt-local model bindings: catalog, need/use declarations, and invocation.
//!
//! A host builds a [`ModelCatalog`] from gateway `GET /v1/models` (or a pinned
//! offline entry). H1 `models.need` resolves a description against that catalog
//! under hard constraints, freezes invocation parameters, and stores the result
//! in the run's crate-private model bindings. H2 `models.use` selects at most
//! one binding per
//! section; H1 `models.always` supplies the prompt-wide default for sections
//! that omit `models.use`. Model-facing sections with neither binding fail with
//! a model-binding failure surfaced through [`crate::RunError`].

use std::num::NonZeroU32;

use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId as PickerToolId};
use serde::Deserialize;
use serde_json::Value;

use crate::Result;
use crate::dialects::{ToolDialectId, ToolsMode};

mod error;
mod resolver;
mod transport;

pub use error::{CompletionError, CompletionErrorKind};
pub use transport::fetch_model_catalog;
pub(crate) use resolver::PickerModelResolver;

/// Stable identity of one catalogued model.
///
/// v0 uses the `"gateway"` namespace plus the caller-facing model name (the
/// gateway `[[model]].name` / OpenAI `id`).
///
/// `#[non_exhaustive]` so the invariant-bearing identity is only ever built
/// through [`ModelId::new`]/[`ModelId::gateway`], never by a struct literal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct ModelId {
    server: String,
    name: String,
}

impl ModelId {
    /// The v0 gateway identity namespace.
    pub const GATEWAY: &'static str = "gateway";

    /// Builds an identity from its server namespace and model name.
    ///
    /// # Errors
    /// Returns [`ModelIdError`] if `server` or `name` is empty or contains a
    /// control character, so an unusable identity is unrepresentable.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::model::ModelId;
    ///
    /// let id = ModelId::new(ModelId::GATEWAY, "claude-sonnet-4-6")?;
    /// assert_eq!(id.server(), "gateway");
    /// assert_eq!(id.name(), "claude-sonnet-4-6");
    /// # Ok::<(), promptforge_core::model::ModelIdError>(())
    /// ```
    pub fn new(
        server: impl Into<String>,
        name: impl Into<String>,
    ) -> std::result::Result<ModelId, ModelIdError> {
        let server = server.into();
        let name = name.into();
        Self::validate("server", &server)?;
        Self::validate("name", &name)?;
        Ok(Self { server, name })
    }

    /// Builds a gateway-namespaced identity from a caller-facing model name.
    ///
    /// # Errors
    /// Returns [`ModelIdError`] if `name` is empty or contains a control
    /// character.
    pub fn gateway(name: impl Into<String>) -> std::result::Result<ModelId, ModelIdError> {
        Self::new(Self::GATEWAY, name)
    }

    /// Builds an identity from components already known to be valid.
    ///
    /// For internal callers reconstructing an identity from an existing
    /// [`ModelId`]'s parts, where [`ModelId::new`]'s validation is redundant.
    pub(crate) fn from_validated(server: impl Into<String>, name: impl Into<String>) -> ModelId {
        ModelId {
            server: server.into(),
            name: name.into(),
        }
    }

    /// Validates one identity component, naming the field in any error.
    fn validate(field: &'static str, value: &str) -> std::result::Result<(), ModelIdError> {
        if value.is_empty() {
            return Err(ModelIdError {
                field,
                reason: "must not be empty",
            });
        }
        if value.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return Err(ModelIdError {
                field,
                reason: "must not contain a control character",
            });
        }
        Ok(())
    }

    /// Returns the identity namespace.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns the caller-facing model name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The reason a [`ModelId`] could not be built from its components.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid model id: {field} {reason}")]
#[non_exhaustive]
pub struct ModelIdError {
    /// Which component was rejected (`server` or `name`).
    field: &'static str,
    /// Why it was rejected.
    reason: &'static str,
}

/// The reason a [`ModelCatalog`] could not be built from its descriptors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ModelCatalogError {
    /// Two descriptors shared one stable [`ModelId`], which would make lookups
    /// ambiguous.
    #[error("duplicate model identity in catalog: {server}/{name}")]
    #[non_exhaustive]
    DuplicateId {
        /// The repeated identity's server namespace.
        server: String,
        /// The repeated identity's model name.
        name: String,
    },
}

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
/// let mode: ThinkingMode = serde_json::from_str("\"switchable\"").expect("valid mode");
/// assert_eq!(mode, ThinkingMode::Switchable);
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
    /// let context = NonZeroU32::new(131_072).expect("non-zero");
    /// let model = ModelDescriptor::new(
    ///     ModelId::gateway("analyst")?,
    ///     "A careful analysis model",
    ///     context,
    ///     ThinkingMode::Switchable,
    /// );
    /// assert_eq!(model.context(), context);
    /// assert_eq!(model.thinking(), ThinkingMode::Switchable);
    /// # Ok::<(), promptforge_core::model::ModelIdError>(())
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
    ///     NonZeroU32::new(32_768).expect("non-zero"),
    ///     ThinkingMode::Never,
    /// )
    /// .with_dialect(ToolDialectId::Gemma3ToolCode);
    /// assert_eq!(model.tool_dialect(), ToolDialectId::Gemma3ToolCode);
    /// assert_eq!(model.tools_mode(), ToolsMode::Emulated);
    /// # Ok::<(), promptforge_core::model::ModelIdError>(())
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

/// Optional hard constraints and invocation parameters from `models.need`.
///
/// `context` and `thinking` filter the catalog. `temperature`, `max_tokens`,
/// and a requested `thinking` switch ride on each completion for the binding.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModelNeedOpts {
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
    /// Sampling temperature, when the need declared one.
    pub(crate) temperature: Option<Temperature>,
    /// Maximum generation tokens, when the need declared one (always non-zero).
    pub(crate) max_tokens: Option<NonZeroU32>,
    /// Thinking switch for `chat_template_kwargs.enable_thinking`, when set.
    pub(crate) thinking: Option<bool>,
}

// No `Eq`: `temperature` is an `f64`, so equality is not reflexive for NaN.

impl From<&ModelNeedOpts> for ModelInvocation {
    fn from(opts: &ModelNeedOpts) -> Self {
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
    ///     .with_max_tokens(NonZeroU32::new(256).expect("non-zero"))
    ///     .with_thinking(false);
    /// # Ok::<(), promptforge_core::model::TemperatureError>(())
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
    always: Option<String>,
}

impl ModelBindings {
    /// Returns bindings in declaration order.
    #[must_use]
    pub(crate) fn bindings(&self) -> &[ModelBinding] {
        &self.bindings
    }

    /// Returns the prompt-wide default alias set by `models.always`, if any.
    #[must_use]
    pub(crate) fn always(&self) -> Option<&str> {
        self.always.as_deref()
    }

    pub(crate) fn binding(&self, alias: &str) -> Option<&ModelBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }

    pub(crate) fn from_parts(bindings: Vec<ModelBinding>, always: Option<String>) -> Self {
        Self { bindings, always }
    }
}

/// Complete live model set for one bind pass.
///
/// `#[non_exhaustive]` so the collision-free catalog invariant is only ever
/// established through [`ModelCatalog::new`]/[`ModelCatalog::empty`].
// No `Eq`: bindings carry `f64` temperatures transitively.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ModelCatalog {
    models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    /// Builds a catalog from descriptors in host order.
    ///
    /// # Errors
    /// Returns [`ModelCatalogError::DuplicateId`] when two descriptors share one
    /// stable [`ModelId`], so an ambiguous catalog is unrepresentable.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU32;
    /// use promptforge_core::model::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
    ///
    /// let ctx = NonZeroU32::new(8_192).expect("non-zero");
    /// let id = ModelId::gateway("small")?;
    /// let catalog = ModelCatalog::new([ModelDescriptor::new(
    ///     id.clone(),
    ///     "A tiny model",
    ///     ctx,
    ///     ThinkingMode::Never,
    /// )])
    /// .expect("unique ids");
    /// assert!(catalog.contains(&id));
    /// assert_eq!(catalog.models().len(), 1);
    /// # Ok::<(), promptforge_core::model::ModelIdError>(())
    /// ```
    pub fn new(
        models: impl IntoIterator<Item = ModelDescriptor>,
    ) -> std::result::Result<ModelCatalog, ModelCatalogError> {
        let models: Vec<ModelDescriptor> = models.into_iter().collect();
        for (index, model) in models.iter().enumerate() {
            if models[..index].iter().any(|prior| prior.id() == model.id()) {
                return Err(ModelCatalogError::DuplicateId {
                    server: model.id().server().to_owned(),
                    name: model.id().name().to_owned(),
                });
            }
        }
        Ok(Self { models })
    }

    /// Builds a catalog from descriptors already known to be collision-free.
    ///
    /// Used by internal callers (like the catalog `filter`) whose inputs are a
    /// subset of an already-validated catalog, where duplicate checking is
    /// redundant.
    pub(crate) fn from_validated(models: Vec<ModelDescriptor>) -> ModelCatalog {
        Self { models }
    }

    /// An empty catalog; every `models.need` resolves as absent.
    #[must_use]
    pub fn empty() -> Self {
        Self::from_validated(Vec::new())
    }

    /// Returns every descriptor.
    #[must_use]
    pub fn models(&self) -> &[ModelDescriptor] {
        &self.models
    }

    /// Returns whether the catalog has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Looks up a descriptor by stable identity.
    #[must_use]
    pub fn get(&self, id: &ModelId) -> Option<&ModelDescriptor> {
        self.models.iter().find(|model| model.id() == id)
    }

    /// Returns whether the catalog contains a descriptor with `id`.
    #[must_use]
    pub fn contains(&self, id: &ModelId) -> bool {
        self.get(id).is_some()
    }

    /// Returns the descriptors satisfying `opts` as borrowed references.
    ///
    /// Unlike [`Self::filter`] this clones nothing (MODEL-017): the semantic
    /// resolver builds its picker directly from these borrowed matches and
    /// selects the resolved descriptor back out of the same borrowed slice.
    #[must_use]
    pub(crate) fn filtered(&self, opts: &ModelNeedOpts) -> Vec<&ModelDescriptor> {
        self.models
            .iter()
            .filter(|model| satisfies_constraints(model, opts))
            .collect()
    }
}

/// Builds a tool-picker [`Catalog`] from borrowed model descriptors.
///
/// The picker's `enriched_text` prefixes the tool name, so vendor model ids
/// must not ride in that name or they drown the capability description.
/// Identity is encoded in the picker id's server field; every entry uses a
/// single neutral, crate-private label. Accepting borrowed descriptors lets a
/// filtered view build a picker without first cloning matches into an owned
/// catalog (MODEL-017).
pub(crate) fn picker_catalog_from<'a>(
    models: impl IntoIterator<Item = &'a ModelDescriptor>,
) -> Catalog {
    Catalog::new(
        models
            .into_iter()
            .map(|model| {
                ToolDescriptor::new(
                    model_to_picker_id(model.id()),
                    model.description.clone(),
                    Value::Object(serde_json::Map::new()),
                )
            })
            .collect(),
    )
}

/// Neutral picker name so `enriched_text` does not inject vendor model ids.
const PICKER_MODEL_LABEL: &str = "model";

/// Separates server and model name inside the picker's server field.
const PICKER_ID_SEPARATOR: char = '\u{1e}';

fn model_to_picker_id(id: &ModelId) -> PickerToolId {
    PickerToolId::new(
        format!("{}{}{}", id.server(), PICKER_ID_SEPARATOR, id.name()),
        PICKER_MODEL_LABEL,
    )
}

pub(crate) fn model_from_picker_id(id: &PickerToolId) -> ModelId {
    match id.server().split_once(PICKER_ID_SEPARATOR) {
        Some((server, name)) if !server.is_empty() && !name.is_empty() => {
            ModelId::from_validated(server, name)
        }
        _ => ModelId::from_validated(id.server(), id.name()),
    }
}

/// Resolves one `models.need` description under optional hard constraints.
pub(crate) trait ModelResolver: Send + Sync {
    /// Resolves `description` with `opts` to a binding identity and invocation.
    ///
    /// # Errors
    /// Returns a core error when the capability cannot be resolved uniquely or
    /// no catalog entry satisfies the constraints.
    fn resolve(&self, description: &str, opts: &ModelNeedOpts) -> Result<ResolvedModel>;
}

impl<F> ModelResolver for F
where
    F: Fn(&str, &ModelNeedOpts) -> Result<ResolvedModel> + Send + Sync,
{
    fn resolve(&self, description: &str, opts: &ModelNeedOpts) -> Result<ResolvedModel> {
        self(description, opts)
    }
}

/// The identity and invocation produced by a successful model resolve.
// No `Eq`: the invocation carries an `f64` temperature.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedModel {
    /// The selected catalog identity.
    pub(crate) id: ModelId,
    /// Frozen per-request fields from the need's opts.
    pub(crate) invocation: ModelInvocation,
    /// The tool dialect from the catalog entry.
    pub(crate) tool_dialect: ToolDialectId,
    /// The catalog context window size in tokens (always non-zero).
    pub(crate) context: NonZeroU32,
}


fn satisfies_constraints(model: &ModelDescriptor, opts: &ModelNeedOpts) -> bool {
    if let Some(min_context) = opts.context
        && model.context < min_context
    {
        return false;
    }
    match opts.thinking {
        Some(true) => matches!(
            model.thinking,
            ThinkingMode::Switchable | ThinkingMode::Always
        ),
        Some(false) => matches!(
            model.thinking,
            ThinkingMode::Switchable | ThinkingMode::Never
        ),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use super::*;
    use crate::Error;
    use crate::lua::{LiveBindingProducer, LuaProgram, SectionVm, ToolBindings, ToolResolver};
    use crate::observe::NullObserver;
    use crate::store::StoreRef;
    use crate::tools::ToolRegistry;
    use serde_json::json;

    const EXECUTION: &str = "model-bind-test";

    fn ctx(window: u32) -> NonZeroU32 {
        NonZeroU32::new(window).expect("test context window is non-zero")
    }

    fn gateway_id(name: &str) -> ModelId {
        ModelId::gateway(name).expect("test model alias is valid")
    }

    fn catalog() -> ModelCatalog {
        ModelCatalog::new([
            ModelDescriptor::new(
                gateway_id("small"),
                "A tiny model",
                ctx(8_192),
                ThinkingMode::Never,
            ),
            ModelDescriptor::new(
                gateway_id("analyst"),
                "A careful analysis model",
                ctx(131_072),
                ThinkingMode::Switchable,
            ),
            ModelDescriptor::new(
                gateway_id("always-think"),
                "Always thinks aloud",
                ctx(64_000),
                ThinkingMode::Always,
            ),
        ])
        .expect("test catalog has unique model ids")
    }

    fn fixture_resolver(description: &str, opts: &ModelNeedOpts) -> Result<ResolvedModel> {
        let catalog = catalog();
        let matches = catalog.filtered(opts);
        let hit = matches
            .iter()
            .find(|model| {
                (description.contains("analysis") && model.id().name() == "analyst")
                    || (description.contains("tiny") && model.id().name() == "small")
            })
            .ok_or_else(|| Error::ModelAbsent {
                capability: description.to_owned(),
            })?;
        Ok(ResolvedModel {
            id: hit.id().clone(),
            invocation: ModelInvocation::from(opts),
            tool_dialect: hit.tool_dialect(),
            context: hit.context(),
        })
    }

    fn resolve_live_declarations_for_test(
        source: &LuaProgram,
        tool_resolver: &dyn ToolResolver,
        model_resolver: &dyn ModelResolver,
        _execution: &str,
        _observer: &dyn crate::observe::Observer,
        _section: &str,
    ) -> Result<(ToolBindings, ModelBindings)> {
        let registry = ToolRegistry::new(std::iter::empty()).expect("unique test registry");
        let producer = LiveBindingProducer::default();
        let lua = Lua::new();
        let result = lua.scope(|scope| {
            producer
                .install(&lua, scope, tool_resolver, &registry, model_resolver)
                .map_err(|error| mlua::Error::external(error.to_string()))?;
            lua.load(source.source()).exec()
        });
        if let Some(error) = producer.take_callback_error()? {
            return Err(error);
        }
        result.map_err(|error| Error::Lua(error.to_string()))?;
        producer.bindings()
    }

    fn section_vm_with_model_bindings(
        _source: &LuaProgram,
        tools: &ToolBindings,
        models: &ModelBindings,
        execution: &str,
        observer: &dyn crate::observe::Observer,
        section: &str,
    ) -> Result<SectionVm> {
        SectionVm::new_for_section(None, tools, models, execution, observer, section)
    }

    #[test]
    fn context_filter_drops_small_windows() {
        let catalog = catalog();
        let matches = catalog.filtered(&ModelNeedOpts {
            context: Some(ctx(40_000)),
            ..ModelNeedOpts::default()
        });
        let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
        assert_eq!(names, ["analyst", "always-think"]);
    }

    #[test]
    fn thinking_false_keeps_never_and_switchable() {
        let catalog = catalog();
        let matches = catalog.filtered(&ModelNeedOpts {
            thinking: Some(false),
            ..ModelNeedOpts::default()
        });
        let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
        assert_eq!(names, ["small", "analyst"]);
    }

    #[test]
    fn thinking_true_keeps_switchable_and_always() {
        let catalog = catalog();
        let matches = catalog.filtered(&ModelNeedOpts {
            thinking: Some(true),
            ..ModelNeedOpts::default()
        });
        let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
        assert_eq!(names, ["analyst", "always-think"]);
    }

    #[test]
    fn same_weights_different_invocation_compare_unequal() {
        let id = gateway_id("analyst");
        let a = ModelBinding::new(
            "cool",
            "careful analysis",
            id.clone(),
            ModelInvocation {
                temperature: Some(Temperature::new(0.0).expect("0.0 is valid")),
                max_tokens: None,
                thinking: Some(false),
            },
            ToolDialectId::OpenAi,
            ctx(131_072),
        );
        let b = ModelBinding::new(
            "warm",
            "careful analysis",
            id,
            ModelInvocation {
                temperature: Some(Temperature::new(0.7).expect("0.7 is valid")),
                max_tokens: None,
                thinking: Some(false),
            },
            ToolDialectId::OpenAi,
            ctx(131_072),
        );
        assert_eq!(a.id(), b.id());
        assert_ne!(a.invocation(), b.invocation());
    }

    #[test]
    fn models_need_resolves_and_use_selects_section_binding() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("analyst", "careful analysis", { thinking = false, temperature = 0, context = 40000 })"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        assert_eq!(models.bindings()[0].id().name(), "analyst");
        assert_eq!(models.bindings()[0].invocation().thinking, Some(false));

        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let prologue = crate::lua::LuaProgram::compile(
            r#"models.use("analyst")"#,
            "prologue",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.run_prologue(&prologue, &NullObserver, "Section")
            .unwrap();
        let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
        assert_eq!(scopes.model.unwrap().alias(), "analyst");
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn no_models_use_or_always_leaves_section_unbound() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("analyst", "careful analysis")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
        assert!(scopes.model.is_none());
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn constraint_filter_makes_need_absent() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("analyst", "careful analysis", { context = 200000 })"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let error = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap_err();
        assert!(matches!(error, Error::ModelAbsent { .. }));
    }

    #[test]
    fn undeclared_models_use_fails_loudly() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("analyst", "careful analysis")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let prologue = crate::lua::LuaProgram::compile(
            r#"models.use("missing")"#,
            "prologue",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        assert!(
            vm.run_prologue(&prologue, &NullObserver, "Section")
                .is_err()
        );
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_records_binding() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("writer", "A tiny model", { thinking = false, temperature = 0 })
               models.always("writer")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (_tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        assert_eq!(models.always(), Some("writer"));
    }

    #[test]
    fn models_always_returns_inspectable_object() {
        let shared = crate::lua::LuaProgram::compile(
            r#"local needed = models.need("writer", "A tiny model", {
                   thinking = false, temperature = 0, max_tokens = 256
               })
               assert(needed.name == "writer")
               assert(needed.model_id == "small")
               assert(needed.description == "A tiny model")
               assert(needed.context == 8192)
               assert(needed.thinking == false)
               assert(needed.temperature == 0)
               assert(needed.max_tokens == 256)
               assert(needed.dialect == "openai")
               local model = models.always("writer")
               assert(model.name == "writer")
               assert(model.model_id == "small")
               assert(model.description == "A tiny model")
               assert(model.context == 8192)
               assert(model.thinking == false)
               assert(model.temperature == 0)
               assert(model.max_tokens == 256)
               assert(model.dialect == "openai")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .expect("models.need/always must return an inspectable Model object");
        assert_eq!(models.always(), Some("writer"));
        assert_eq!(models.bindings()[0].context().get(), 8_192);

        let vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .expect("section install must expose the same inspectable Model object");
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_without_prior_need_fails() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let error = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(msg.contains("not declared"), "unexpected error: {msg}");
    }

    #[test]
    fn models_always_duplicate_fails() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("writer", "A tiny model")
               models.always("writer")
               models.always("writer")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let error = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(msg.contains("at most once"), "unexpected error: {msg}");
    }

    #[test]
    fn models_always_installs_exactly() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("writer", "A tiny model")
               models.always("writer")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
        assert_eq!(
            scopes.model.as_ref().map(ModelBinding::alias),
            Some("writer")
        );
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_provides_completion_options_without_use() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("writer", "A tiny model", { thinking = false, temperature = 0 })
               models.always("writer")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
        let opts = scopes.model.as_ref().map(ModelBinding::completion_options);
        let expected = CompletionOptions {
            model: "small".to_owned(),
            temperature: Some(Temperature::new(0.0).expect("0.0 is valid")),
            max_tokens: None,
            thinking: Some(false),
            tool_dialect: ToolDialectId::OpenAi,
        };
        assert_eq!(opts, Some(expected));
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_use_overrides_always() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("writer", "A tiny model", { thinking = false })
               models.need("analyst", "careful analysis", { thinking = true })
               models.always("writer")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let prologue = crate::lua::LuaProgram::compile(
            r#"models.use("analyst")"#,
            "prologue",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.run_prologue(&prologue, &NullObserver, "Section")
            .unwrap();
        let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
        assert_eq!(
            scopes.model.as_ref().map(ModelBinding::alias),
            Some("analyst")
        );
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_from_h2_prologue_fails() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("writer", "A tiny model")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let prologue = crate::lua::LuaProgram::compile(
            r#"models.always("writer")"#,
            "prologue",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        let result = vm.run_prologue(&prologue, &NullObserver, "Section");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("only available during live H1 execution"),
            "unexpected error: {msg}"
        );
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_multi_arg_records_need_and_always() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer", "A tiny model", { thinking = false, temperature = 0 })"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (_tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        assert_eq!(models.always(), Some("writer"));
        assert!(models.binding("writer").is_some());
    }

    #[test]
    fn models_always_multi_arg_two_args() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer", "A tiny model")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (_tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        assert_eq!(models.always(), Some("writer"));
        assert!(models.binding("writer").is_some());
    }

    #[test]
    fn models_always_multi_arg_provides_completion_options() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer", "A tiny model", { thinking = false, temperature = 0 })"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
        let opts = scopes.model.as_ref().map(ModelBinding::completion_options);
        let expected = CompletionOptions {
            model: "small".to_owned(),
            temperature: Some(Temperature::new(0.0).expect("0.0 is valid")),
            max_tokens: None,
            thinking: Some(false),
            tool_dialect: ToolDialectId::OpenAi,
        };
        assert_eq!(opts, Some(expected));
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_multi_arg_installs_exactly() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer", "A tiny model", { thinking = false })"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = section_vm_with_model_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        vm.inject_host("", &json!({}), &StoreRef::memory(), None)
            .unwrap();
        let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
        assert_eq!(
            scopes.model.as_ref().map(ModelBinding::alias),
            Some("writer")
        );
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_multi_arg_and_single_arg_cannot_both_be_called() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("analyst", "careful analysis")
               models.always("writer", "A tiny model")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (_tools, models) = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        assert_eq!(models.always(), Some("writer"));

        // Now verify that a second always (single-arg) after multi-arg always fails.
        let shared2 = crate::lua::LuaProgram::compile(
            r#"models.always("writer", "A tiny model")
               models.always("writer")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let error = resolve_live_declarations_for_test(
            &shared2,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(msg.contains("at most once"), "unexpected error: {msg}");
    }

    #[test]
    fn models_always_multi_arg_duplicate_alias_fails() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("writer", "A tiny model")
               models.always("writer", "A tiny model")"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let error = resolve_live_declarations_for_test(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap_err();
        let msg = error.to_string();
        assert!(
            msg.contains("duplicate")
                || msg.contains("Duplicate")
                || msg.contains("declared more than once"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn descriptor_with_dialect_sets_tools_mode() {
        let descriptor = ModelDescriptor::new(
            gateway_id("gemma-local"),
            "A Gemma model",
            ctx(32_768),
            ThinkingMode::Never,
        )
        .with_dialect(ToolDialectId::Gemma3ToolCode);
        assert_eq!(descriptor.tool_dialect(), ToolDialectId::Gemma3ToolCode);
        assert_eq!(
            descriptor.tools_mode(),
            crate::dialects::ToolsMode::Emulated
        );
    }

    #[test]
    fn descriptor_default_dialect_is_openai_native() {
        let descriptor = ModelDescriptor::new(
            gateway_id("remote"),
            "A remote model",
            ctx(8_192),
            ThinkingMode::Never,
        );
        assert_eq!(descriptor.tool_dialect(), ToolDialectId::OpenAi);
        assert_eq!(descriptor.tools_mode(), crate::dialects::ToolsMode::Native);
    }

    #[test]
    fn model_invocation_equality_is_not_reflexive_for_nan() {
        // Documents why these float-bearing types intentionally do not implement
        // `Eq`: a NaN temperature is not equal to itself.
        let nan = ModelInvocation {
            temperature: Some(Temperature(f64::NAN)),
            max_tokens: None,
            thinking: None,
        };
        assert_ne!(nan, nan.clone());
    }

    #[test]
    fn completion_options_equality_is_not_reflexive_for_nan() {
        // `CompletionOptions` carries an `Option<Temperature>` (an `f64`
        // newtype) temperature, so it must not implement `Eq`: a NaN temperature
        // is not equal to itself, which would violate the reflexivity `Eq`
        // promises. This test would fail to compile if `Eq` were (re)added. A NaN
        // cannot enter through the public validated setter; the field is set with
        // an in-crate `Temperature(NaN)` (private tuple) to prove the soundness
        // reason `Eq` is withheld.
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

    #[test]
    fn model_id_rejects_empty_and_control_characters() {
        assert!(ModelId::gateway("").is_err());
        assert!(ModelId::new("", "name").is_err());
        assert!(ModelId::new("server", "").is_err());
        assert!(ModelId::new("server", "na\nme").is_err());
        assert!(ModelId::gateway("valid-alias").is_ok());
    }

    #[test]
    fn model_catalog_rejects_duplicate_ids() {
        let descriptor = |name: &str| {
            ModelDescriptor::new(gateway_id(name), "d", ctx(8_192), ThinkingMode::Never)
        };
        let err = ModelCatalog::new([descriptor("dup"), descriptor("dup")])
            .expect_err("a catalog with duplicate ids must be rejected");
        assert!(matches!(err, ModelCatalogError::DuplicateId { .. }));
        assert!(ModelCatalog::new([descriptor("a"), descriptor("b")]).is_ok());
    }

    #[test]
    fn binding_dialect_propagates_to_completion_options() {
        let binding = ModelBinding::new(
            "gemma",
            "a local gemma model",
            gateway_id("gemma-local"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            ToolDialectId::Gemma3ToolCode,
            ctx(8_192),
        );
        let opts = binding.completion_options();
        assert_eq!(opts.tool_dialect, ToolDialectId::Gemma3ToolCode);
    }

    #[test]
    fn binding_construction_is_atomic_with_dialect_and_context() {
        let binding = ModelBinding::new(
            "remote",
            "a remote model",
            gateway_id("remote"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            ToolDialectId::OpenAi,
            ctx(64_000),
        );
        let opts = binding.completion_options();
        assert_eq!(opts.tool_dialect, ToolDialectId::OpenAi);
        assert_eq!(binding.context().get(), 64_000);
    }

    #[tokio::test]
    async fn fetch_model_catalog_rejects_a_wire_tools_mode_that_contradicts_the_dialect() {
        use axum::Router;
        use axum::routing::get;

        // MODEL-008: a wire `tools_mode` is validated against the mode derived
        // from `tool_dialect`. An OpenAI (native) dialect paired with an
        // `emulated` wire mode is contradictory and must be refused as malformed
        // rather than silently keeping one of the two.
        async fn models() -> axum::Json<serde_json::Value> {
            axum::Json(serde_json::json!({
                "data": [{
                    "id": "remote",
                    "description": "a remote model",
                    "context": 8192,
                    "thinking": "never",
                    "tool_dialect": "openai",
                    "tools_mode": "emulated"
                }]
            }))
        }
        let app = Router::new().route("/models", get(models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("a contradictory wire tools_mode must be rejected");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        assert!(
            err.to_string().contains("contradicts"),
            "the rejection must name the contradiction, got {err}"
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_bounds_and_reports_non_success_body() {
        use axum::Router;
        use axum::routing::get;

        async fn models() -> (axum::http::StatusCode, String) {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "e".repeat(transport::MAX_CATALOG_ERROR_BODY * 4),
            )
        }
        let app = Router::new().route("/models", get(models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("a 500 response must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::Backend);
        let msg = err.to_string();
        assert!(
            msg.len() < transport::MAX_CATALOG_ERROR_BODY + 128,
            "the error-path body must be bounded, got {} bytes",
            msg.len()
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_bounds_an_oversized_success_body() {
        use axum::Router;
        use axum::routing::get;

        // A 200 response whose body exceeds the success cap must be refused
        // BEFORE decoding, not buffered unbounded. The body is deliberately not
        // valid JSON: the bound must trip first, regardless of contents.
        async fn models() -> (axum::http::StatusCode, String) {
            let oversized = usize::try_from(transport::MAX_CATALOG_BODY).unwrap() + 1;
            (axum::http::StatusCode::OK, "e".repeat(oversized))
        }
        let app = Router::new().route("/models", get(models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("an oversized success body must be refused");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        assert!(
            err.to_string().contains("exceeds"),
            "the bound must report the size limit, got {err}"
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_preserves_the_json_decode_source() {
        use axum::Router;
        use axum::routing::get;

        // MODEL-009: a 200 body that is not a valid model list is classified as
        // MalformedResponse, and the underlying `serde_json::Error` survives as
        // the error-chain `#[source]` rather than being flattened into the text.
        async fn models() -> (axum::http::StatusCode, String) {
            (axum::http::StatusCode::OK, "{ this is not json".to_owned())
        }
        let app = Router::new().route("/models", get(models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("an undecodable body must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        let source =
            std::error::Error::source(&err).expect("the decode error must be a preserved source");
        assert!(
            source.downcast_ref::<serde_json::Error>().is_some(),
            "the preserved source must be the JSON decode error, got {source}"
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_preserves_a_body_read_failure_source() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // MODEL-010: a non-success response whose body cannot be fully read
        // (the server promises a large body then drops the connection) must
        // surface as a typed transport failure that keeps the `reqwest::Error`
        // as its `#[source]`, not display text.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let header = "HTTP/1.1 500 Internal Server Error\r\n\
                     Content-Length: 1000000\r\n\r\n";
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(b"abc").await;
                // Socket drops here: the promised body never completes.
            }
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("a truncated error body must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::Transport);
        assert_eq!(err.status(), Some(500));
        let source =
            std::error::Error::source(&err).expect("the read failure must be a preserved source");
        assert!(
            source.downcast_ref::<reqwest::Error>().is_some(),
            "the preserved source must be the reqwest read error, got {source}"
        );
    }
}
