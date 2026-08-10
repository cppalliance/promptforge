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

use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId as PickerToolId, ToolPicker};
use serde::Deserialize;
use serde_json::Value;

use crate::dialects::{ToolDialectId, ToolsMode};
use crate::{Error, Result};

/// A stable, matchable classification of a [`CompletionError`].
///
/// `#[non_exhaustive]` so new kinds do not break a caller's `match`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionErrorKind {
    /// The HTTP request failed at the transport layer (connection, timeout).
    Transport,
    /// The backend returned a non-success status.
    Backend,
    /// The backend response could not be decoded or was structurally invalid.
    MalformedResponse,
    /// The model returned neither non-empty tool calls nor non-empty text.
    EmptyReply,
    /// Gateway access was explicitly disabled by the host.
    Disabled,
    /// The client could not be configured (missing environment, bad endpoint,
    /// or dialect selection).
    Config,
}

/// The error returned by the gateway transport ([`crate::client::GatewayClient`]
/// completion and catalog calls) and [`fetch_model_catalog`].
///
/// Carries a stable [`kind`](CompletionError::kind) classifier plus the
/// `is_retryable`/`is_timeout`/`status` predicates, and preserves the underlying
/// transport cause through [`std::error::Error::source`]. `#[non_exhaustive]`
/// and not constructible outside the crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct CompletionError {
    inner: Error,
}

impl CompletionError {
    /// Returns the stable classification of this failure.
    #[must_use]
    pub fn kind(&self) -> CompletionErrorKind {
        match &self.inner {
            Error::Http(_) => CompletionErrorKind::Transport,
            Error::Backend { .. } => CompletionErrorKind::Backend,
            Error::MalformedResponse(_) => CompletionErrorKind::MalformedResponse,
            Error::EmptyModelReply { .. } => CompletionErrorKind::EmptyReply,
            Error::GatewayDisabled => CompletionErrorKind::Disabled,
            _ => CompletionErrorKind::Config,
        }
    }

    /// Returns the backend HTTP status, when the failure was a backend status.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match &self.inner {
            Error::Backend { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Returns `true` when the transport failure was a timeout.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        match &self.inner {
            Error::Http(source) => source
                .downcast_ref::<reqwest::Error>()
                .is_some_and(reqwest::Error::is_timeout),
            _ => false,
        }
    }

    /// Returns `true` when retrying may succeed (transient transport or 5xx).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match &self.inner {
            Error::Http(_) | Error::MalformedResponse(_) => true,
            Error::Backend { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

impl std::fmt::Display for CompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for CompletionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.inner)
    }
}

impl From<Error> for CompletionError {
    fn from(inner: Error) -> Self {
        CompletionError { inner }
    }
}

impl From<CompletionError> for Error {
    fn from(error: CompletionError) -> Self {
        error.inner
    }
}

/// Stable identity of one catalogued model.
///
/// v0 uses the `"gateway"` namespace plus the caller-facing model name (the
/// gateway `[[model]].name` / OpenAI `id`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// Whether a catalogued model can emit thinking tokens.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    id: ModelId,
    description: String,
    context: NonZeroU32,
    thinking: ThinkingMode,
    tool_dialect: ToolDialectId,
    tools_mode: ToolsMode,
}

impl ModelDescriptor {
    /// Builds a descriptor from its identity and catalog fields.
    ///
    /// The context window is a [`NonZeroU32`], so a zero-token window is
    /// unrepresentable. Defaults `tool_dialect` to [`ToolDialectId::OpenAi`] and
    /// `tools_mode` to [`ToolsMode::Native`]. Use [`Self::with_dialect`] to
    /// override.
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
            tools_mode: ToolsMode::Native,
        }
    }

    /// Sets the tool dialect and derives `tools_mode` from it.
    #[must_use]
    pub fn with_dialect(mut self, dialect: ToolDialectId) -> Self {
        self.tool_dialect = dialect;
        self.tools_mode = dialect.tools_mode();
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
    #[must_use]
    pub fn tools_mode(&self) -> ToolsMode {
        self.tools_mode
    }
}

/// Optional hard constraints and invocation parameters from `models.need`.
///
/// `context` and `thinking` filter the catalog. `temperature`, `max_tokens`,
/// and a requested `thinking` switch ride on each completion for the binding.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModelNeedOpts {
    /// When set, filters models by thinking capability and freezes the switch.
    pub(crate) thinking: Option<bool>,
    /// Minimum context window size in tokens.
    pub(crate) context: Option<u32>,
    /// Sampling temperature for every complete under this binding.
    pub(crate) temperature: Option<f64>,
    /// Maximum generation tokens for every complete under this binding.
    pub(crate) max_tokens: Option<u32>,
}

impl PartialEq for ModelNeedOpts {
    fn eq(&self, other: &Self) -> bool {
        self.thinking == other.thinking
            && self.context == other.context
            && self.max_tokens == other.max_tokens
            && match (self.temperature, other.temperature) {
                (None, None) => true,
                (Some(left), Some(right)) => left.to_bits() == right.to_bits(),
                _ => false,
            }
    }
}

// No `Eq`: `temperature` is an `f64`, so equality is not reflexive for NaN.

/// Frozen per-request fields carried by a resolved model binding.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ModelInvocation {
    /// Sampling temperature, when the need declared one.
    pub(crate) temperature: Option<f64>,
    /// Maximum generation tokens, when the need declared one.
    pub(crate) max_tokens: Option<u32>,
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
    context: u32,
}

impl ModelBinding {
    /// Builds a binding from its parts (tests and resolvers).
    ///
    /// Defaults `tool_dialect` to [`ToolDialectId::OpenAi`] and `context` to
    /// `0`. Use [`Self::with_dialect`] and [`Self::with_context`] to override.
    #[must_use]
    pub(crate) fn new(
        alias: impl Into<String>,
        description: impl Into<String>,
        id: ModelId,
        invocation: ModelInvocation,
    ) -> Self {
        Self {
            alias: alias.into(),
            description: description.into(),
            id,
            invocation,
            tool_dialect: ToolDialectId::OpenAi,
            context: 0,
        }
    }

    /// Sets the tool dialect on this binding.
    #[must_use]
    pub(crate) fn with_dialect(mut self, dialect: ToolDialectId) -> Self {
        self.tool_dialect = dialect;
        self
    }

    /// Sets the catalog context window size on this binding.
    #[must_use]
    pub(crate) fn with_context(mut self, context: u32) -> Self {
        self.context = context;
        self
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

    /// Returns the catalog context window size in tokens.
    #[must_use]
    pub(crate) fn context(&self) -> u32 {
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
    /// Sampling temperature.
    pub(crate) temperature: Option<f64>,
    /// Maximum generation tokens.
    pub(crate) max_tokens: Option<u32>,
    /// When set, emits `chat_template_kwargs.enable_thinking`.
    pub(crate) thinking: Option<bool>,
    /// Which tool-calling dialect to use for this completion.
    pub(crate) tool_dialect: ToolDialectId,
}

impl Eq for CompletionOptions {}

impl CompletionOptions {
    /// Builds options for `model` under the given tool-calling `dialect`, with no
    /// temperature, token cap, or thinking switch.
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

    /// Sets the sampling temperature.
    #[must_use]
    pub fn with_temperature(mut self, temperature: f64) -> CompletionOptions {
        self.temperature = Some(temperature);
        self
    }

    /// Sets the maximum generation tokens.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: u32) -> CompletionOptions {
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
// No `Eq`: bindings carry `f64` temperatures transitively.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelCatalog {
    models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    /// Builds a catalog from descriptors in host order.
    ///
    /// # Errors
    /// Returns [`ModelCatalogError::DuplicateId`] when two descriptors share one
    /// stable [`ModelId`], so an ambiguous catalog is unrepresentable.
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

    /// Filters by hard constraints from `opts`.
    #[must_use]
    pub(crate) fn filter(&self, opts: &ModelNeedOpts) -> Self {
        let models = self
            .models
            .iter()
            .filter(|model| satisfies_constraints(model, opts))
            .cloned()
            .collect();
        Self::from_validated(models)
    }

    /// Builds a tool-picker [`Catalog`] from model descriptions for semantic resolve.
    ///
    /// The picker's `enriched_text` prefixes the tool name, so vendor model ids
    /// must not ride in that name or they drown the capability description.
    /// Identity is encoded in the picker id's server field; every entry uses a
    /// single neutral, crate-private label.
    #[must_use]
    pub(crate) fn to_picker_catalog(&self) -> Catalog {
        Catalog::new(
            self.models
                .iter()
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

fn model_from_picker_id(id: &PickerToolId) -> ModelId {
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
    /// The catalog context window size in tokens.
    pub(crate) context: u32,
}

/// Wire shape of one entry from gateway `GET /v1/models`.
#[derive(Debug, Deserialize)]
struct ModelsListEntry {
    id: String,
    description: String,
    context: u32,
    thinking: ThinkingMode,
    #[serde(default = "default_tool_dialect")]
    tool_dialect: ToolDialectId,
}

fn default_tool_dialect() -> ToolDialectId {
    ToolDialectId::OpenAi
}

/// Wire shape of gateway `GET /v1/models`.
#[derive(Debug, Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelsListEntry>,
}

/// The largest gateway error body kept for a catalog-fetch diagnostic, in bytes.
const MAX_CATALOG_ERROR_BODY: usize = 2000;

/// Reads at most `limit` bytes of a non-success response body, stopping early so
/// an oversized error body cannot exhaust memory, and preserving a read failure
/// as an explicit diagnostic rather than an empty string.
async fn read_error_body_bounded(mut response: reqwest::Response, limit: usize) -> String {
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let take = (limit - buffer.len()).min(chunk.len());
                buffer.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    break;
                }
            }
            Ok(None) => break,
            Err(source) => {
                return format!("(backend response body could not be read: {source})");
            }
        }
    }
    if buffer.is_empty() {
        return "(empty body)".to_owned();
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Fetches a [`ModelCatalog`] from a bearer-authed gateway `/models` endpoint.
///
/// `base_url` is the OpenAI-shaped API root (for example `http://127.0.0.1:8081/v1`).
///
/// # Errors
/// Returns a [`CompletionError`] whose [`kind`](CompletionError::kind) is
/// `Transport` on transport failure, `Backend` on a non-success status, and
/// `MalformedResponse` when the body is not a model list.
pub async fn fetch_model_catalog(
    base_url: &str,
    token: &str,
) -> std::result::Result<ModelCatalog, CompletionError> {
    let base = base_url.trim_end_matches('/');
    let http = reqwest::Client::new();
    let response = http
        .get(format!("{base}/models"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(Error::http)?;
    let status = response.status();
    if !status.is_success() {
        // The error body is external, so bound the read (MODEL-010: no unbounded
        // buffering) and preserve a read failure instead of masking it as empty.
        let body = read_error_body_bounded(response, MAX_CATALOG_ERROR_BODY).await;
        return Err(CompletionError::from(Error::Backend {
            status: status.as_u16(),
            body,
        }));
    }
    // A body that does not decode as a model list is a malformed response, not a
    // transport failure - matching this function's documented error contract.
    let list: ModelsListResponse = response.json().await.map_err(|error| {
        CompletionError::from(Error::MalformedResponse(format!(
            "model list response was not valid JSON: {error}"
        )))
    })?;
    let mut descriptors = Vec::with_capacity(list.data.len());
    for entry in list.data {
        let id = ModelId::gateway(entry.id).map_err(|error| {
            CompletionError::from(Error::MalformedResponse(format!(
                "model catalog entry has an invalid id: {error}"
            )))
        })?;
        let context = NonZeroU32::new(entry.context).ok_or_else(|| {
            CompletionError::from(Error::MalformedResponse(format!(
                "model {} declares a zero-token context window",
                id.name()
            )))
        })?;
        descriptors.push(
            ModelDescriptor::new(id, entry.description, context, entry.thinking)
                .with_dialect(entry.tool_dialect),
        );
    }
    ModelCatalog::new(descriptors).map_err(|error| {
        CompletionError::from(Error::MalformedResponse(format!(
            "gateway returned an inconsistent model catalog: {error}"
        )))
    })
}

/// Builds a [`ToolPicker`] over `catalog` by reusing `base`'s embedder.
///
/// # Errors
/// Returns [`Error::ModelBind`] when the picker cannot index the catalog.
pub(crate) fn model_picker_from(base: &ToolPicker, catalog: &ModelCatalog) -> Result<ToolPicker> {
    base.rebuild(catalog.to_picker_catalog())
        .map_err(|error| Error::ModelBind {
            capability: String::new(),
            detail: error.to_string(),
        })
}

/// Resolver that filters the catalog, then semantically resolves via a picker.
#[derive(Debug)]
pub(crate) struct PickerModelResolver<'a> {
    catalog: &'a ModelCatalog,
    picker: &'a ToolPicker,
}

impl<'a> PickerModelResolver<'a> {
    /// Borrows a catalog and a picker built over that catalog's descriptors.
    #[must_use]
    pub(crate) fn new(catalog: &'a ModelCatalog, picker: &'a ToolPicker) -> Self {
        Self { catalog, picker }
    }
}

impl ModelResolver for PickerModelResolver<'_> {
    fn resolve(&self, description: &str, opts: &ModelNeedOpts) -> Result<ResolvedModel> {
        let filtered = self.catalog.filter(opts);
        if filtered.is_empty() {
            return Err(Error::ModelAbsent {
                capability: description.to_owned(),
            });
        }
        let picker = self
            .picker
            .rebuild(filtered.to_picker_catalog())
            .map_err(|error| Error::ModelBind {
                capability: description.to_owned(),
                detail: error.to_string(),
            })?;
        match picker.resolve(description) {
            Ok(promptforge_tool_picker::Outcome::Bind(tool)) => {
                let id = model_from_picker_id(tool.id());
                // The picker was rebuilt from `filtered`, so a selected id absent
                // from it is an encoding/consistency fault, not a bind. Fail
                // explicitly instead of fabricating OpenAI + zero-context metadata.
                let descriptor = filtered.get(&id).ok_or_else(|| Error::ModelBind {
                    capability: description.to_owned(),
                    detail: format!(
                        "picker selected model {}/{} which is absent from the filtered live catalog",
                        id.server(),
                        id.name()
                    ),
                })?;
                Ok(ResolvedModel {
                    id,
                    invocation: ModelInvocation::from(opts),
                    tool_dialect: descriptor.tool_dialect(),
                    context: descriptor.context().get(),
                })
            }
            Ok(promptforge_tool_picker::Outcome::Absent) => Err(Error::ModelAbsent {
                capability: description.to_owned(),
            }),
            Ok(promptforge_tool_picker::Outcome::Duplicate(group)) => Err(Error::ModelDuplicate {
                capability: description.to_owned(),
                candidates: group
                    .iter()
                    .map(|tool| model_from_picker_id(tool.id()))
                    .collect(),
            }),
            Ok(promptforge_tool_picker::Outcome::Ambiguous(group)) => Err(Error::ModelAmbiguous {
                capability: description.to_owned(),
                candidates: group
                    .iter()
                    .map(|tool| model_from_picker_id(tool.id()))
                    .collect(),
            }),
            Ok(_) => Err(Error::ModelBind {
                capability: description.to_owned(),
                detail: "the picker reported an unrecognized outcome".to_owned(),
            }),
            Err(error) => Err(Error::ModelBind {
                capability: description.to_owned(),
                detail: error.to_string(),
            }),
        }
    }
}

fn satisfies_constraints(model: &ModelDescriptor, opts: &ModelNeedOpts) -> bool {
    if let Some(min_context) = opts.context
        && model.context.get() < min_context
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
        let filtered = catalog().filter(opts);
        let hit = filtered
            .models()
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
            context: hit.context().get(),
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
        let filtered = catalog().filter(&ModelNeedOpts {
            context: Some(40_000),
            ..ModelNeedOpts::default()
        });
        let names: Vec<_> = filtered.models().iter().map(|m| m.id().name()).collect();
        assert_eq!(names, ["analyst", "always-think"]);
    }

    #[test]
    fn thinking_false_keeps_never_and_switchable() {
        let filtered = catalog().filter(&ModelNeedOpts {
            thinking: Some(false),
            ..ModelNeedOpts::default()
        });
        let names: Vec<_> = filtered.models().iter().map(|m| m.id().name()).collect();
        assert_eq!(names, ["small", "analyst"]);
    }

    #[test]
    fn thinking_true_keeps_switchable_and_always() {
        let filtered = catalog().filter(&ModelNeedOpts {
            thinking: Some(true),
            ..ModelNeedOpts::default()
        });
        let names: Vec<_> = filtered.models().iter().map(|m| m.id().name()).collect();
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
                temperature: Some(0.0),
                max_tokens: None,
                thinking: Some(false),
            },
        );
        let b = ModelBinding::new(
            "warm",
            "careful analysis",
            id,
            ModelInvocation {
                temperature: Some(0.7),
                max_tokens: None,
                thinking: Some(false),
            },
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
        assert_eq!(models.bindings()[0].context(), 8_192);

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
            temperature: Some(0.0),
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
            temperature: Some(0.0),
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
            temperature: Some(f64::NAN),
            max_tokens: None,
            thinking: None,
        };
        assert_ne!(nan, nan.clone());
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
    fn binding_with_dialect_propagates_to_completion_options() {
        let binding = ModelBinding::new(
            "gemma",
            "a local gemma model",
            gateway_id("gemma-local"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
        )
        .with_dialect(ToolDialectId::Gemma3ToolCode);
        let opts = binding.completion_options();
        assert_eq!(opts.tool_dialect, ToolDialectId::Gemma3ToolCode);
    }

    #[test]
    fn binding_default_dialect_is_openai() {
        let binding = ModelBinding::new(
            "remote",
            "a remote model",
            gateway_id("remote"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
        );
        let opts = binding.completion_options();
        assert_eq!(opts.tool_dialect, ToolDialectId::OpenAi);
    }

    #[test]
    fn models_list_entry_parses_dialect_fields() {
        let json = serde_json::json!({
            "id": "gemma-local",
            "description": "A Gemma model",
            "context": 32768,
            "thinking": "never",
            "tool_dialect": "gemma3_tool_code",
            "tools_mode": "emulated"
        });
        let entry: ModelsListEntry = serde_json::from_value(json).unwrap();
        assert_eq!(entry.tool_dialect, ToolDialectId::Gemma3ToolCode);
        // tools_mode is derived from the dialect at runtime, not read from the wire.
        assert_eq!(
            entry.tool_dialect.tools_mode(),
            crate::dialects::ToolsMode::Emulated
        );
    }

    #[test]
    fn models_list_entry_defaults_to_openai_native() {
        let json = serde_json::json!({
            "id": "remote",
            "description": "A remote model",
            "context": 8192,
            "thinking": "never"
        });
        let entry: ModelsListEntry = serde_json::from_value(json).unwrap();
        assert_eq!(entry.tool_dialect, ToolDialectId::OpenAi);
        assert_eq!(
            entry.tool_dialect.tools_mode(),
            crate::dialects::ToolsMode::Native
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_bounds_and_reports_non_success_body() {
        use axum::Router;
        use axum::routing::get;

        async fn models() -> (axum::http::StatusCode, String) {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "e".repeat(MAX_CATALOG_ERROR_BODY * 4),
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
            msg.len() < MAX_CATALOG_ERROR_BODY + 128,
            "the error-path body must be bounded, got {} bytes",
            msg.len()
        );
    }
}
