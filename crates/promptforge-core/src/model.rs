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
pub(crate) use resolver::PickerModelResolver;
pub use transport::fetch_model_catalog;

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
mod tests;
