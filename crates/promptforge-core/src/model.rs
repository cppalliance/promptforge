//! Prompt-local model bindings: catalog, need/use declarations, and invocation.
//!
//! A host builds a [`ModelCatalog`] from gateway `GET /v1/models` (or a pinned
//! offline entry). H1 `models.need` resolves a description against that catalog
//! under hard constraints, freezes invocation parameters, and stores the result
//! in [`ModelBindings`]. H2 `models.use` selects at most one binding per
//! section; H1 `models.always` supplies the prompt-wide default for sections
//! that omit `models.use`. Model-facing sections with neither binding fail with
//! [`crate::Error::ModelRequired`].

use promptforge_tool_picker::{Catalog, ToolDescriptor, ToolId as PickerToolId, ToolPicker};
use serde::Deserialize;
use serde_json::Value;

use crate::dialects::{ToolDialectId, ToolsMode};
use crate::{Error, Result};

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
    /// # Examples
    ///
    /// ```
    /// use promptforge_core::model::ModelId;
    ///
    /// let id = ModelId::new(ModelId::GATEWAY, "claude-sonnet-4-6");
    /// assert_eq!(id.server(), "gateway");
    /// assert_eq!(id.name(), "claude-sonnet-4-6");
    /// ```
    #[must_use]
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            name: name.into(),
        }
    }

    /// Builds a gateway-namespaced identity from a caller-facing model name.
    #[must_use]
    pub fn gateway(name: impl Into<String>) -> Self {
        Self::new(Self::GATEWAY, name)
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

/// One catalogued model with bind-time metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    id: ModelId,
    description: String,
    context: u32,
    thinking: ThinkingMode,
    tool_dialect: ToolDialectId,
    tools_mode: ToolsMode,
}

impl ModelDescriptor {
    /// Builds a descriptor from its identity and catalog fields.
    ///
    /// Defaults `tool_dialect` to [`ToolDialectId::OpenAi`] and `tools_mode` to
    /// [`ToolsMode::Native`]. Use [`Self::with_dialect`] to override.
    #[must_use]
    pub fn new(
        id: ModelId,
        description: impl Into<String>,
        context: u32,
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

    /// Returns the context window size in tokens.
    #[must_use]
    pub fn context(&self) -> u32 {
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
pub struct ModelNeedOpts {
    /// When set, filters models by thinking capability and freezes the switch.
    pub thinking: Option<bool>,
    /// Minimum context window size in tokens.
    pub context: Option<u32>,
    /// Sampling temperature for every complete under this binding.
    pub temperature: Option<f64>,
    /// Maximum generation tokens for every complete under this binding.
    pub max_tokens: Option<u32>,
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

impl Eq for ModelNeedOpts {}

/// Frozen per-request fields carried by a resolved model binding.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInvocation {
    /// Sampling temperature, when the need declared one.
    pub temperature: Option<f64>,
    /// Maximum generation tokens, when the need declared one.
    pub max_tokens: Option<u32>,
    /// Thinking switch for `chat_template_kwargs.enable_thinking`, when set.
    pub thinking: Option<bool>,
}

impl Eq for ModelInvocation {}

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelBinding {
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
    pub fn new(
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
    pub fn with_dialect(mut self, dialect: ToolDialectId) -> Self {
        self.tool_dialect = dialect;
        self
    }

    /// Sets the catalog context window size on this binding.
    #[must_use]
    pub fn with_context(mut self, context: u32) -> Self {
        self.context = context;
        self
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

    /// Returns the selected stable identity.
    #[must_use]
    pub fn id(&self) -> &ModelId {
        &self.id
    }

    /// Returns the frozen per-request fields.
    #[must_use]
    pub fn invocation(&self) -> &ModelInvocation {
        &self.invocation
    }

    /// Returns the tool dialect for this binding.
    #[must_use]
    pub fn tool_dialect(&self) -> ToolDialectId {
        self.tool_dialect
    }

    /// Returns the catalog context window size in tokens.
    #[must_use]
    pub fn context(&self) -> u32 {
        self.context
    }

    /// Builds [`CompletionOptions`] for every complete under this binding.
    #[must_use]
    pub fn completion_options(&self) -> CompletionOptions {
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
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionOptions {
    /// The caller-facing model name sent on the wire.
    pub model: String,
    /// Sampling temperature.
    pub temperature: Option<f64>,
    /// Maximum generation tokens.
    pub max_tokens: Option<u32>,
    /// When set, emits `chat_template_kwargs.enable_thinking`.
    pub thinking: Option<bool>,
    /// Which tool-calling dialect to use for this completion.
    pub tool_dialect: ToolDialectId,
}

impl Eq for CompletionOptions {}

/// Immutable prompt-level model bindings from one H1 declaration pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelBindings {
    bindings: Vec<ModelBinding>,
    declarations: Vec<ModelDeclaration>,
    always: Option<String>,
}

impl ModelBindings {
    /// Returns bindings in declaration order.
    #[must_use]
    pub fn bindings(&self) -> &[ModelBinding] {
        &self.bindings
    }

    /// Returns whether any model was declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Returns the prompt-wide default alias set by `models.always`, if any.
    #[must_use]
    pub fn always(&self) -> Option<&str> {
        self.always.as_deref()
    }

    pub(crate) fn binding(&self, alias: &str) -> Option<&ModelBinding> {
        self.bindings.iter().find(|binding| binding.alias == alias)
    }

    pub(crate) fn declarations(&self) -> &[ModelDeclaration] {
        &self.declarations
    }

    pub(crate) fn from_parts(
        bindings: Vec<ModelBinding>,
        declarations: Vec<ModelDeclaration>,
        always: Option<String>,
    ) -> Self {
        Self {
            bindings,
            declarations,
            always,
        }
    }
}

/// Exact H1 declaration recorded for section replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelDeclaration {
    Need {
        alias: String,
        description: String,
        opts: ModelNeedOpts,
    },
    Always(String),
}

/// Complete live model set for one bind pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelCatalog {
    models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    /// Builds a catalog from descriptors in host order.
    #[must_use]
    pub fn new(models: impl IntoIterator<Item = ModelDescriptor>) -> Self {
        Self {
            models: models.into_iter().collect(),
        }
    }

    /// An empty catalog; every `models.need` resolves as absent.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(std::iter::empty())
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

    /// Filters by hard constraints from `opts`.
    #[must_use]
    pub fn filter(&self, opts: &ModelNeedOpts) -> Self {
        let models = self
            .models
            .iter()
            .filter(|model| satisfies_constraints(model, opts))
            .cloned()
            .collect();
        Self { models }
    }

    /// Builds a tool-picker [`Catalog`] from model descriptions for semantic resolve.
    ///
    /// The picker's `enriched_text` prefixes the tool name, so vendor model ids
    /// must not ride in that name or they drown the capability description.
    /// Identity is encoded in the picker id's server field; every entry uses the
    /// neutral label [`PICKER_MODEL_LABEL`].
    #[must_use]
    pub fn to_picker_catalog(&self) -> Catalog {
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
            ModelId::new(server, name)
        }
        _ => ModelId::new(id.server(), id.name()),
    }
}

/// Registry view of the live catalog for membership checks after resolve.
#[derive(Debug, Clone)]
pub struct ModelRegistry<'a> {
    catalog: &'a ModelCatalog,
}

impl<'a> ModelRegistry<'a> {
    /// Borrows a catalog as the live registry.
    #[must_use]
    pub fn new(catalog: &'a ModelCatalog) -> Self {
        Self { catalog }
    }

    /// Returns whether `id` is present in the live catalog.
    #[must_use]
    pub fn contains(&self, id: &ModelId) -> bool {
        self.catalog.get(id).is_some()
    }

    /// Returns the borrowed catalog.
    #[must_use]
    pub fn catalog(&self) -> &'a ModelCatalog {
        self.catalog
    }
}

/// Resolves one `models.need` description under optional hard constraints.
pub trait ModelResolver: Send + Sync {
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    /// The selected catalog identity.
    pub id: ModelId,
    /// Frozen per-request fields from the need's opts.
    pub invocation: ModelInvocation,
    /// The tool dialect from the catalog entry.
    pub tool_dialect: ToolDialectId,
    /// The catalog context window size in tokens.
    pub context: u32,
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
    /// Parsed for wire-shape completeness; the runtime derives tools_mode from
    /// tool_dialect via [`ToolDialectId::tools_mode`].
    #[serde(default = "default_tools_mode")]
    #[allow(dead_code)]
    tools_mode: ToolsMode,
}

fn default_tool_dialect() -> ToolDialectId {
    ToolDialectId::OpenAi
}

fn default_tools_mode() -> ToolsMode {
    ToolsMode::Native
}

/// Wire shape of gateway `GET /v1/models`.
#[derive(Debug, Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelsListEntry>,
}

/// Fetches a [`ModelCatalog`] from a bearer-authed gateway `/models` endpoint.
///
/// `base_url` is the OpenAI-shaped API root (for example `http://127.0.0.1:8081/v1`).
///
/// # Errors
/// Returns [`Error::Http`] on transport failure, [`Error::Backend`] on a
/// non-success status, and [`Error::MalformedResponse`] when the body is not a
/// model list.
pub async fn fetch_model_catalog(base_url: &str, token: &str) -> Result<ModelCatalog> {
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
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(2000).collect();
        return Err(Error::Backend {
            status: status.as_u16(),
            body: if body.is_empty() {
                "(empty body)".to_owned()
            } else {
                body
            },
        });
    }
    let list: ModelsListResponse = response.json().await.map_err(Error::http)?;
    Ok(ModelCatalog::new(list.data.into_iter().map(|entry| {
        ModelDescriptor::new(
            ModelId::gateway(entry.id),
            entry.description,
            entry.context,
            entry.thinking,
        )
        .with_dialect(entry.tool_dialect)
    })))
}

/// Builds a [`ToolPicker`] over `catalog` by reusing `base`'s embedder.
///
/// # Errors
/// Returns [`Error::ModelBind`] when the picker cannot index the catalog.
pub fn model_picker_from(base: &ToolPicker, catalog: &ModelCatalog) -> Result<ToolPicker> {
    base.rebuild(catalog.to_picker_catalog())
        .map_err(|error| Error::ModelBind {
            capability: String::new(),
            detail: error.to_string(),
        })
}

/// Resolver that filters the catalog, then semantically resolves via a picker.
#[derive(Debug)]
pub struct PickerModelResolver<'a> {
    catalog: &'a ModelCatalog,
    picker: &'a ToolPicker,
}

impl<'a> PickerModelResolver<'a> {
    /// Borrows a catalog and a picker built over that catalog's descriptors.
    #[must_use]
    pub fn new(catalog: &'a ModelCatalog, picker: &'a ToolPicker) -> Self {
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
                let id = model_from_picker_id(&tool.id);
                let descriptor = filtered.get(&id);
                let dialect =
                    descriptor.map_or(ToolDialectId::OpenAi, ModelDescriptor::tool_dialect);
                let context = descriptor.map_or(0, ModelDescriptor::context);
                Ok(ResolvedModel {
                    id,
                    invocation: ModelInvocation::from(opts),
                    tool_dialect: dialect,
                    context,
                })
            }
            Ok(promptforge_tool_picker::Outcome::Absent) => Err(Error::ModelAbsent {
                capability: description.to_owned(),
            }),
            Ok(promptforge_tool_picker::Outcome::Duplicate(tools)) => Err(Error::ModelDuplicate {
                capability: description.to_owned(),
                candidates: tools
                    .iter()
                    .map(|tool| model_from_picker_id(&tool.id))
                    .collect(),
            }),
            Ok(promptforge_tool_picker::Outcome::Ambiguous(tools)) => Err(Error::ModelAmbiguous {
                capability: description.to_owned(),
                candidates: tools
                    .iter()
                    .map(|tool| model_from_picker_id(&tool.id))
                    .collect(),
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

/// Catalog entry used by core-tests scenario fixtures for model binding.
///
/// Context is a large switchable window so `models.need` can filter and
/// request thinking without depending on a live `/v1/models` fetch.
#[must_use]
pub fn pinned_qwen_dev_catalog(model_alias: &str) -> ModelCatalog {
    ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway(model_alias),
        "A careful analysis model suited to structured reasoning and long-context review",
        131_072,
        ThinkingMode::Switchable,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::{SectionVm, bind_shared_declarations};
    use crate::observe::NullObserver;
    use crate::store::StoreRef;
    use serde_json::json;

    const EXECUTION: &str = "model-bind-test";

    fn catalog() -> ModelCatalog {
        ModelCatalog::new([
            ModelDescriptor::new(
                ModelId::gateway("small"),
                "A tiny model",
                8_192,
                ThinkingMode::Never,
            ),
            ModelDescriptor::new(
                ModelId::gateway("analyst"),
                "A careful analysis model",
                131_072,
                ThinkingMode::Switchable,
            ),
            ModelDescriptor::new(
                ModelId::gateway("always-think"),
                "Always thinks aloud",
                64_000,
                ThinkingMode::Always,
            ),
        ])
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
            context: hit.context(),
        })
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
        let id = ModelId::gateway("analyst");
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
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

        let mut vm = SectionVm::new_with_shared_bindings(
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
            1,
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = SectionVm::new_with_shared_bindings(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let error = bind_shared_declarations(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = SectionVm::new_with_shared_bindings(
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
            1,
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (_tools, models) = bind_shared_declarations(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
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

        let vm = SectionVm::new_with_shared_bindings(
            &shared,
            &tools,
            &models,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .expect("replay must return the same inspectable Model object");
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_without_prior_need_fails() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer")"#,
            "shared",
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let error = bind_shared_declarations(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let error = bind_shared_declarations(
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
    fn models_always_replays_exactly() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.need("writer", "A tiny model")
               models.always("writer")"#,
            "shared",
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = SectionVm::new_with_shared_bindings(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = SectionVm::new_with_shared_bindings(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = SectionVm::new_with_shared_bindings(
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
            1,
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = SectionVm::new_with_shared_bindings(
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
            1,
            EXECUTION,
            &NullObserver,
            "Section",
        )
        .unwrap();
        let result = vm.run_prologue(&prologue, &NullObserver, "Section");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("only available during H1"),
            "unexpected error: {msg}"
        );
        vm.teardown(&NullObserver, "Section");
    }

    #[test]
    fn models_always_multi_arg_records_need_and_always() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer", "A tiny model", { thinking = false, temperature = 0 })"#,
            "shared",
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (_tools, models) = bind_shared_declarations(
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
        assert_eq!(models.declarations().len(), 2);
        assert!(matches!(
            &models.declarations()[0],
            ModelDeclaration::Need { alias, .. } if alias == "writer"
        ));
        assert!(matches!(
            &models.declarations()[1],
            ModelDeclaration::Always(alias) if alias == "writer"
        ));
    }

    #[test]
    fn models_always_multi_arg_two_args() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer", "A tiny model")"#,
            "shared",
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (_tools, models) = bind_shared_declarations(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = SectionVm::new_with_shared_bindings(
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
    fn models_always_multi_arg_replays_exactly() {
        let shared = crate::lua::LuaProgram::compile(
            r#"models.always("writer", "A tiny model", { thinking = false })"#,
            "shared",
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (tools, models) = bind_shared_declarations(
            &shared,
            &tool_resolver,
            &fixture_resolver,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let mut vm = SectionVm::new_with_shared_bindings(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let (_tools, models) = bind_shared_declarations(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let error = bind_shared_declarations(
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
            1,
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
        let tool_resolver =
            |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
        let error = bind_shared_declarations(
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
            ModelId::gateway("gemma-local"),
            "A Gemma model",
            32_768,
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
            ModelId::gateway("remote"),
            "A remote model",
            8_192,
            ThinkingMode::Never,
        );
        assert_eq!(descriptor.tool_dialect(), ToolDialectId::OpenAi);
        assert_eq!(descriptor.tools_mode(), crate::dialects::ToolsMode::Native);
    }

    #[test]
    fn binding_with_dialect_propagates_to_completion_options() {
        let binding = ModelBinding::new(
            "gemma",
            "a local gemma model",
            ModelId::gateway("gemma-local"),
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
            ModelId::gateway("remote"),
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
        assert_eq!(entry.tools_mode, crate::dialects::ToolsMode::Emulated);
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
        assert_eq!(entry.tools_mode, crate::dialects::ToolsMode::Native);
    }
}
