//! Stable parse-and-run facade.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

pub use promptforge_tool_picker::ToolId;
use promptforge_tool_picker::ToolPicker;

/// The PromptForge language version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EngineVersion(u32);

impl EngineVersion {
    /// The language version implemented by this crate.
    pub const SUPPORTED: Self = Self(1);

    /// Returns the major version.
    #[must_use]
    pub const fn major(self) -> u32 {
        self.0
    }
}

/// A validated, immutable prompt.
#[derive(Debug, Clone)]
pub struct Prompt(crate::parser::Prompt);

impl Prompt {
    /// Parses PromptForge source.
    ///
    /// # Errors
    /// Returns an error when frontmatter, Markdown structure, or embedded Lua is invalid.
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        crate::parser::Prompt::parse(source, "parse", &crate::observe::NullObserver)
            .map(Self)
            .map_err(ParseError::from)
    }

    /// Returns the prompt name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0.frontmatter.name
    }

    /// Returns the prompt description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.0.frontmatter.description
    }

    /// Returns the declared engine version.
    #[must_use]
    pub fn version(&self) -> Option<EngineVersion> {
        self.0.frontmatter.promptforge.map(EngineVersion)
    }

    /// Returns the H1 title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.0.title
    }

    /// Iterates over top-level sections.
    #[must_use]
    pub fn sections(&self) -> Sections<'_> {
        Sections(self.0.sections.iter())
    }
}

/// An iterator over top-level sections.
#[derive(Debug, Clone)]
pub struct Sections<'a>(std::slice::Iter<'a, crate::parser::Section>);

impl<'a> Iterator for Sections<'a> {
    type Item = SectionRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(SectionRef)
    }
}

/// A read-only section view.
#[derive(Debug, Clone, Copy)]
pub struct SectionRef<'a>(&'a crate::parser::Section);

impl SectionRef<'_> {
    /// Returns the heading text.
    #[must_use]
    pub fn heading(&self) -> &str {
        &self.0.name
    }

    /// Returns the heading level.
    #[must_use]
    pub fn level(&self) -> HeadingLevel {
        match self.0.level {
            2 => HeadingLevel::H2,
            3 => HeadingLevel::H3,
            4 => HeadingLevel::H4,
            5 => HeadingLevel::H5,
            _ => HeadingLevel::H6,
        }
    }

    /// Returns whether the section contains model-facing prose.
    #[must_use]
    pub fn has_prose(&self) -> bool {
        !self.0.prose().trim().is_empty()
    }
}

/// A supported Markdown section level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HeadingLevel {
    /// Level two.
    H2,
    /// Level three.
    H3,
    /// Level four.
    H4,
    /// Level five.
    H5,
    /// Level six.
    H6,
}

/// Detects a declared PromptForge version.
///
/// # Errors
/// Returns an error when a declared frontmatter block is malformed.
pub fn detect_version(source: &str) -> Result<Option<EngineVersion>, ParseError> {
    if !source.trim_start_matches('\u{feff}').starts_with("---") {
        return Ok(None);
    }
    let value = crate::parser::promptforge_version(source).map(EngineVersion);
    if value.is_none()
        && source
            .lines()
            .any(|line| line.trim_start().starts_with("promptforge:"))
    {
        return Err(ParseError::message(
            "invalid promptforge version declaration",
        ));
    }
    Ok(value)
}

/// Validated JSON text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Json(Box<str>);

impl FromStr for Json {
    type Err = JsonError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_str::<serde_json::Value>(value)
            .map_err(|_| JsonError)
            .map(|_| Self(value.into()))
    }
}

impl Json {
    fn from_value(value: &serde_json::Value) -> Self {
        Self(value.to_string().into())
    }

    /// Returns the original JSON text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated object-shaped JSON text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JsonObject(Json);

impl FromStr for JsonObject {
    type Err = JsonError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parsed = serde_json::from_str::<serde_json::Value>(value).map_err(|_| JsonError)?;
        if !parsed.is_object() {
            return Err(JsonError);
        }
        Ok(Self(Json(value.into())))
    }
}

impl JsonObject {
    /// Returns this value as general JSON.
    #[must_use]
    pub fn as_json(&self) -> &Json {
        &self.0
    }

    /// Returns the original JSON text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn value(&self) -> serde_json::Value {
        serde_json::from_str(self.as_str())
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
    }
}

/// An invalid JSON boundary value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid json value")]
pub struct JsonError;

/// A configured semantic capability resolver.
#[derive(Debug)]
pub struct CapabilityResolver(ToolPicker);

impl From<ToolPicker> for CapabilityResolver {
    fn from(value: ToolPicker) -> Self {
        Self(value)
    }
}

/// An immutable tool advertisement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    id: ToolId,
    wire_name: Box<str>,
    description: Box<str>,
    parameters: JsonObject,
}

impl ToolSpec {
    /// Builds and validates a tool advertisement.
    ///
    /// # Errors
    /// Returns an error for blank or protocol-invalid names, blank descriptions, or schemas.
    pub fn try_new(
        id: ToolId,
        wire_name: impl Into<Box<str>>,
        description: impl Into<Box<str>>,
        parameters: JsonObject,
    ) -> Result<Self, ToolSpecError> {
        let wire_name = wire_name.into();
        let description = description.into();
        if !valid_component(id.server()) || !valid_component(id.name()) || !valid_wire(&wire_name) {
            return Err(ToolSpecError);
        }
        if description.trim().is_empty() {
            return Err(ToolSpecError);
        }
        Ok(Self {
            id,
            wire_name,
            description,
            parameters,
        })
    }

    /// Returns the stable identity.
    #[must_use]
    pub fn id(&self) -> &ToolId {
        &self.id
    }
    /// Returns the model wire name.
    #[must_use]
    pub fn wire_name(&self) -> &str {
        &self.wire_name
    }
    /// Returns the model-facing description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }
    /// Returns the parameters schema.
    #[must_use]
    pub fn parameters(&self) -> &JsonObject {
        &self.parameters
    }
}

fn valid_component(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn valid_wire(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && value.len() <= 64
}

/// An invalid tool advertisement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid tool specification")]
pub struct ToolSpecError;

/// Trust attached to every tool output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OutputTrust {
    /// Host-established trusted output.
    Trusted,
    /// External data that must be guarded before model use.
    Untrusted,
}

/// Text returned by a tool with mandatory trust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    text: String,
    trust: OutputTrust,
}

impl ToolOutput {
    /// Builds a tool output.
    #[must_use]
    pub fn new(text: impl Into<String>, trust: OutputTrust) -> Self {
        Self {
            text: text.into(),
            trust,
        }
    }
    /// Returns the text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Returns the trust classification.
    #[must_use]
    pub fn trust(&self) -> OutputTrust {
        self.trust
    }
}

/// Borrowed input to one tool call.
#[derive(Debug, Clone, Copy)]
pub struct ToolRequest<'a> {
    arguments: &'a JsonObject,
    cancellation: &'a Cancellation,
}

impl ToolRequest<'_> {
    /// Returns validated arguments.
    #[must_use]
    pub fn arguments(&self) -> &JsonObject {
        self.arguments
    }
    /// Returns the run cancellation token.
    #[must_use]
    pub fn cancellation(&self) -> &Cancellation {
        self.cancellation
    }
}

/// A host-provided executable tool.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Returns the immutable advertisement.
    fn spec(&self) -> &ToolSpec;
    /// Executes one call.
    async fn call(&self, request: ToolRequest<'_>) -> Result<ToolOutput, ToolCallError>;
}

/// A validated ordered tool set.
#[derive(Clone)]
pub struct ToolSet {
    tools: Arc<[Arc<dyn Tool>]>,
}

impl fmt::Debug for ToolSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolSet").field("len", &self.len()).finish()
    }
}

impl ToolSet {
    /// Builds a set, rejecting duplicate ids and wire names.
    ///
    /// # Errors
    /// Returns an error when an identity or wire name occurs more than once.
    pub fn try_new(tools: impl IntoIterator<Item = Arc<dyn Tool>>) -> Result<Self, ToolSetError> {
        let tools: Vec<_> = tools.into_iter().collect();
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        for tool in &tools {
            if !ids.insert(tool.spec().id().clone())
                || !names.insert(tool.spec().wire_name().to_owned())
            {
                return Err(ToolSetError);
            }
        }
        Ok(Self {
            tools: tools.into(),
        })
    }
    /// Returns the number of tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }
    /// Returns whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
    /// Iterates over tools.
    #[must_use]
    pub fn iter(&self) -> Tools<'_> {
        Tools(self.tools.iter())
    }
    /// Looks up a tool by identity.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|tool| tool.spec().id() == id)
            .map(AsRef::as_ref)
    }
}

impl Default for ToolSet {
    fn default() -> Self {
        Self {
            tools: Arc::from([]),
        }
    }
}

/// An iterator over tools.
#[derive(Clone)]
pub struct Tools<'a>(std::slice::Iter<'a, Arc<dyn Tool>>);

impl fmt::Debug for Tools<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tools").finish_non_exhaustive()
    }
}

impl<'a> Iterator for Tools<'a> {
    type Item = &'a dyn Tool;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(AsRef::as_ref)
    }
}

/// A duplicate in a tool set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("duplicate tool identity or wire name")]
pub struct ToolSetError;

/// Stable model identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(Box<str>);

impl FromStr for ModelId {
    type Err = ModelIdError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            Err(ModelIdError)
        } else {
            Ok(Self(value.into()))
        }
    }
}

impl ModelId {
    /// Returns the identity text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An invalid model identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid model identity")]
pub struct ModelIdError;

/// Model thinking support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ThinkingMode {
    /// Never thinks.
    Never,
    /// Always thinks.
    Always,
    /// Host-switchable.
    Switchable,
}

/// Model tool-call protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ToolProtocol {
    /// OpenAI function calls.
    OpenAi,
    /// Gemma tool-code fences.
    Gemma3ToolCode,
}

/// A validated model catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    id: ModelId,
    description: Box<str>,
    context_tokens: NonZeroU32,
    thinking: ThinkingMode,
    tool_protocol: ToolProtocol,
}

impl ModelDescriptor {
    /// Builds a descriptor.
    ///
    /// # Errors
    /// Returns an error when the description is blank.
    pub fn try_new(
        id: ModelId,
        description: impl Into<Box<str>>,
        context_tokens: NonZeroU32,
        thinking: ThinkingMode,
        tool_protocol: ToolProtocol,
    ) -> Result<Self, ModelDescriptorError> {
        let description = description.into();
        if description.trim().is_empty() {
            return Err(ModelDescriptorError);
        }
        Ok(Self {
            id,
            description,
            context_tokens,
            thinking,
            tool_protocol,
        })
    }
    /// Returns its identity.
    #[must_use]
    pub fn id(&self) -> &ModelId {
        &self.id
    }
    /// Returns its description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }
    /// Returns its context size.
    #[must_use]
    pub fn context_tokens(&self) -> NonZeroU32 {
        self.context_tokens
    }
    /// Returns thinking support.
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }
    /// Returns the tool protocol.
    #[must_use]
    pub fn tool_protocol(&self) -> ToolProtocol {
        self.tool_protocol
    }
}

/// An invalid model descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid model descriptor")]
pub struct ModelDescriptorError;

/// A unique model catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelCatalog {
    models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    /// Builds a catalog.
    ///
    /// # Errors
    /// Returns an error for duplicate identities.
    pub fn try_new(
        models: impl IntoIterator<Item = ModelDescriptor>,
    ) -> Result<Self, ModelCatalogError> {
        let models: Vec<_> = models.into_iter().collect();
        let mut ids = BTreeSet::new();
        if models.iter().any(|model| !ids.insert(model.id.clone())) {
            return Err(ModelCatalogError);
        }
        Ok(Self { models })
    }
    /// Returns whether empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
    /// Returns its length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }
    /// Iterates over models.
    #[must_use]
    pub fn iter(&self) -> Models<'_> {
        Models(self.models.iter())
    }
    /// Looks up a model.
    #[must_use]
    pub fn get(&self, id: &ModelId) -> Option<&ModelDescriptor> {
        self.models.iter().find(|model| model.id() == id)
    }
}

/// An iterator over models.
#[derive(Debug, Clone)]
pub struct Models<'a>(std::slice::Iter<'a, ModelDescriptor>);
impl<'a> Iterator for Models<'a> {
    type Item = &'a ModelDescriptor;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// Duplicate model identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("duplicate model identity")]
pub struct ModelCatalogError;

/// A validated HTTP or HTTPS gateway API root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayEndpoint(reqwest::Url);

impl TryFrom<&str> for GatewayEndpoint {
    type Error = GatewayConfigError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let url = reqwest::Url::parse(value).map_err(|_| GatewayConfigError)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.cannot_be_a_base()
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(GatewayConfigError);
        }
        Ok(Self(url))
    }
}

/// A redacted gateway credential.
#[derive(Clone)]
pub struct GatewayCredential(Box<str>);
impl GatewayCredential {
    /// Validates a credential.
    ///
    /// # Errors
    /// Returns an error when empty or control-bearing.
    pub fn try_new(value: impl Into<Box<str>>) -> Result<Self, GatewayConfigError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_control) {
            Err(GatewayConfigError)
        } else {
            Ok(Self(value))
        }
    }
}
impl fmt::Debug for GatewayCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GatewayCredential(<redacted>)")
    }
}

/// Bounded gateway HTTP policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpPolicy {
    timeout: Duration,
    max_response_bytes: NonZeroUsize,
}
impl HttpPolicy {
    /// Builds a policy.
    ///
    /// # Errors
    /// Returns an error for a zero timeout.
    pub fn try_new(
        timeout: Duration,
        max_response_bytes: NonZeroUsize,
    ) -> Result<Self, GatewayConfigError> {
        if timeout.is_zero() {
            Err(GatewayConfigError)
        } else {
            Ok(Self {
                timeout,
                max_response_bytes,
            })
        }
    }
}
impl Default for HttpPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            max_response_bytes: NonZeroUsize::new(8 * 1024 * 1024).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// A reusable gateway client.
#[derive(Clone)]
pub struct GatewayClient {
    endpoint: GatewayEndpoint,
    credential: GatewayCredential,
    policy: HttpPolicy,
}
impl fmt::Debug for GatewayClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayClient")
            .field("endpoint", &self.endpoint)
            .field("credential", &"<redacted>")
            .field("policy", &self.policy)
            .finish()
    }
}
impl GatewayClient {
    /// Builds a gateway client.
    #[must_use]
    pub fn new(
        endpoint: GatewayEndpoint,
        credential: GatewayCredential,
        policy: HttpPolicy,
    ) -> Self {
        Self {
            endpoint,
            credential,
            policy,
        }
    }
    /// Fetches and validates the model catalog.
    ///
    /// # Errors
    /// Returns a gateway error for transport, status, size, decode, or validation failures.
    pub async fn models(&self) -> Result<ModelCatalog, GatewayError> {
        let client = reqwest::Client::builder()
            .timeout(self.policy.timeout)
            .build()
            .map_err(|_| GatewayError)?;
        let url = self.endpoint.0.join("models").map_err(|_| GatewayError)?;
        let response = client
            .get(url)
            .bearer_auth(&*self.credential.0)
            .send()
            .await
            .map_err(|_| GatewayError)?;
        if !response.status().is_success() {
            return Err(GatewayError);
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.policy.max_response_bytes.get() as u64)
        {
            return Err(GatewayError);
        }
        let bytes = response.bytes().await.map_err(|_| GatewayError)?;
        if bytes.len() > self.policy.max_response_bytes.get() {
            return Err(GatewayError);
        }
        #[derive(serde::Deserialize)]
        struct Response {
            data: Vec<Entry>,
        }
        #[derive(serde::Deserialize)]
        struct Entry {
            id: String,
            description: String,
            context: u32,
            thinking: crate::model::ThinkingMode,
            #[serde(default)]
            tool_dialect: Option<crate::dialects::ToolDialectId>,
        }
        let decoded: Response = serde_json::from_slice(&bytes).map_err(|_| GatewayError)?;
        let models = decoded
            .data
            .into_iter()
            .map(|entry| {
                let id = entry.id.parse().map_err(|_| GatewayError)?;
                let context = NonZeroU32::new(entry.context).ok_or(GatewayError)?;
                let thinking = match entry.thinking {
                    crate::model::ThinkingMode::Never => ThinkingMode::Never,
                    crate::model::ThinkingMode::Always => ThinkingMode::Always,
                    crate::model::ThinkingMode::Switchable => ThinkingMode::Switchable,
                };
                let protocol = match entry
                    .tool_dialect
                    .unwrap_or(crate::dialects::ToolDialectId::OpenAi)
                {
                    crate::dialects::ToolDialectId::OpenAi => ToolProtocol::OpenAi,
                    crate::dialects::ToolDialectId::Gemma3ToolCode => ToolProtocol::Gemma3ToolCode,
                };
                ModelDescriptor::try_new(id, entry.description, context, thinking, protocol)
                    .map_err(|_| GatewayError)
            })
            .collect::<Result<Vec<_>, _>>()?;
        ModelCatalog::try_new(models).map_err(|_| GatewayError)
    }
    fn legacy(&self) -> crate::client::GatewayClient {
        crate::client::GatewayClient::new(self.endpoint.0.as_str(), self.credential.0.to_string())
    }
}

/// An invalid gateway configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid gateway configuration")]
pub struct GatewayConfigError;
/// A gateway operation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("gateway operation")]
pub struct GatewayError;

/// A validated logical store path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorePath(Box<str>);
impl TryFrom<&str> for StorePath {
    type Error = StorePathError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.starts_with('/')
            || value.contains('\\')
            || value
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
            || value.chars().any(char::is_control)
        {
            Err(StorePathError)
        } else {
            Ok(Self(value.into()))
        }
    }
}
impl StorePath {
    /// Returns the normalized path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated bounded glob pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobPattern(Box<str>);
impl TryFrom<&str> for GlobPattern {
    type Error = StorePathError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 1024
            || value.contains('\\')
            || value.chars().any(char::is_control)
        {
            Err(StorePathError)
        } else {
            Ok(Self(value.into()))
        }
    }
}

/// A host-provided synchronous store backend.
pub trait StoreBackend: Send + Sync {
    /// Writes a file.
    fn write(&mut self, path: &StorePath, contents: &str) -> Result<(), StoreError>;
    /// Appends a file.
    fn append(&mut self, path: &StorePath, contents: &str) -> Result<(), StoreError>;
    /// Reads a file.
    fn read(&self, path: &StorePath) -> Result<String, StoreError>;
    /// Replaces one unique anchor.
    fn replace(&mut self, path: &StorePath, old: &str, new: &str) -> Result<(), StoreError>;
    /// Deletes a file.
    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError>;
    /// Lists matching paths.
    fn glob(&self, pattern: &GlobPattern) -> Result<Vec<StorePath>, StoreError>;
    /// Tests existence without swallowing backend errors.
    fn exists(&self, path: &StorePath) -> Result<bool, StoreError>;
}

/// In-memory store backend.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore(BTreeMap<StorePath, String>);
impl StoreBackend for MemoryStore {
    fn write(&mut self, path: &StorePath, contents: &str) -> Result<(), StoreError> {
        self.0.insert(path.clone(), contents.into());
        Ok(())
    }
    fn append(&mut self, path: &StorePath, contents: &str) -> Result<(), StoreError> {
        self.0.entry(path.clone()).or_default().push_str(contents);
        Ok(())
    }
    fn read(&self, path: &StorePath) -> Result<String, StoreError> {
        self.0.get(path).cloned().ok_or(StoreError)
    }
    fn replace(&mut self, path: &StorePath, old: &str, new: &str) -> Result<(), StoreError> {
        if old.is_empty() {
            return Err(StoreError);
        }
        let value = self.0.get(path).ok_or(StoreError)?;
        if value.matches(old).count() != 1 {
            return Err(StoreError);
        }
        let value = value.replacen(old, new, 1);
        self.0.insert(path.clone(), value);
        Ok(())
    }
    fn delete(&mut self, path: &StorePath) -> Result<(), StoreError> {
        self.0.remove(path).map(|_| ()).ok_or(StoreError)
    }
    fn glob(&self, pattern: &GlobPattern) -> Result<Vec<StorePath>, StoreError> {
        let pattern = pattern.0.replace("**", "*");
        Ok(self
            .0
            .keys()
            .filter(|path| wildcard(&pattern, path.as_str()))
            .cloned()
            .collect())
    }
    fn exists(&self, path: &StorePath) -> Result<bool, StoreError> {
        Ok(self.0.contains_key(path))
    }
}

fn wildcard(pattern: &str, text: &str) -> bool {
    let (mut p, mut t, mut star, mut mark) = (0, 0, None, 0);
    let pbytes = pattern.as_bytes();
    let tbytes = text.as_bytes();
    while t < tbytes.len() {
        if p < pbytes.len() && pbytes[p] == tbytes[t] {
            p += 1;
            t += 1;
        } else if p < pbytes.len() && pbytes[p] == b'*' {
            star = Some(p);
            p += 1;
            mark = t;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pbytes[p..].iter().all(|byte| *byte == b'*')
}

/// A shareable store handle.
#[derive(Clone)]
pub struct StoreHandle(Arc<std::sync::Mutex<Box<dyn StoreBackend>>>);
impl fmt::Debug for StoreHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreHandle").finish_non_exhaustive()
    }
}
impl StoreHandle {
    /// Wraps a backend.
    #[must_use]
    pub fn new(backend: impl StoreBackend + 'static) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Box::new(backend))))
    }
    /// Creates an in-memory store.
    #[must_use]
    pub fn memory() -> Self {
        Self::new(MemoryStore::default())
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Box<dyn StoreBackend>>, StoreError> {
        self.0.lock().map_err(|_| StoreError)
    }
    /// Writes.
    pub fn write(&self, p: &StorePath, c: &str) -> Result<(), StoreError> {
        self.lock()?.write(p, c)
    }
    /// Appends.
    pub fn append(&self, p: &StorePath, c: &str) -> Result<(), StoreError> {
        self.lock()?.append(p, c)
    }
    /// Reads.
    pub fn read(&self, p: &StorePath) -> Result<String, StoreError> {
        self.lock()?.read(p)
    }
    /// Reads numbered lines.
    pub fn read_lines(&self, p: &StorePath) -> Result<String, StoreError> {
        self.read(p).map(|s| {
            s.lines()
                .enumerate()
                .map(|(i, l)| format!("{}| {l}", i + 1))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }
    /// Replaces.
    pub fn replace(&self, p: &StorePath, o: &str, n: &str) -> Result<(), StoreError> {
        self.lock()?.replace(p, o, n)
    }
    /// Deletes.
    pub fn delete(&self, p: &StorePath) -> Result<(), StoreError> {
        self.lock()?.delete(p)
    }
    /// Globs.
    pub fn glob(&self, p: &GlobPattern) -> Result<Vec<StorePath>, StoreError> {
        self.lock()?.glob(p)
    }
    /// Tests existence.
    pub fn exists(&self, p: &StorePath) -> Result<bool, StoreError> {
        self.lock()?.exists(p)
    }
}

/// Invalid path or pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid store path or pattern")]
pub struct StorePathError;
/// Store operation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("store operation")]
pub struct StoreError;

/// Explicit one-way cancellation.
#[derive(Debug, Clone, Default)]
pub struct Cancellation(crate::cancel::CancelHandle);
impl Cancellation {
    /// Creates an uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Cancels all clones.
    pub fn cancel(&self) {
        self.0.cancel();
    }
    /// Returns whether cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    /// Waits for cancellation.
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}

/// Validated execution correlation id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExecutionId(Box<str>);
impl TryFrom<&str> for ExecutionId {
    type Error = ExecutionIdError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
            Err(ExecutionIdError)
        } else {
            Ok(Self(value.into()))
        }
    }
}
/// Invalid execution id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid execution identity")]
pub struct ExecutionIdError;

/// Bounded run resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    fanout_concurrency: NonZeroUsize,
    max_fanout_items: NonZeroUsize,
    max_tool_iterations: NonZeroUsize,
    max_recursion: NonZeroUsize,
    lua_instruction_budget: NonZeroU64,
    lua_memory_bytes: NonZeroUsize,
    max_log_events: NonZeroUsize,
    max_log_bytes: NonZeroUsize,
}
impl Default for RunLimits {
    fn default() -> Self {
        Self {
            fanout_concurrency: NonZeroUsize::new(8).unwrap_or(NonZeroUsize::MIN),
            max_fanout_items: NonZeroUsize::new(256).unwrap_or(NonZeroUsize::MIN),
            max_tool_iterations: NonZeroUsize::new(24).unwrap_or(NonZeroUsize::MIN),
            max_recursion: NonZeroUsize::new(8).unwrap_or(NonZeroUsize::MIN),
            lua_instruction_budget: NonZeroU64::new(10_000_000).unwrap_or(NonZeroU64::MIN),
            lua_memory_bytes: NonZeroUsize::new(64 * 1024 * 1024).unwrap_or(NonZeroUsize::MIN),
            max_log_events: NonZeroUsize::new(1024).unwrap_or(NonZeroUsize::MIN),
            max_log_bytes: NonZeroUsize::new(1024 * 1024).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// One owned, valid run request.
pub struct RunRequest {
    prompt: Prompt,
    execution: ExecutionId,
    resolver: CapabilityResolver,
    models: ModelCatalog,
    input: String,
    tools: ToolSet,
    store: StoreHandle,
    gateway: Option<GatewayClient>,
    cancellation: Cancellation,
    observer: Arc<dyn Observer>,
    capture: Option<Arc<dyn SensitiveCapture>>,
    limits: RunLimits,
}
impl fmt::Debug for RunRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunRequest")
            .field("execution", &self.execution)
            .finish_non_exhaustive()
    }
}
impl RunRequest {
    /// Builds a request.
    #[must_use]
    pub fn new(
        prompt: Prompt,
        execution: ExecutionId,
        resolver: CapabilityResolver,
        models: ModelCatalog,
    ) -> Self {
        Self {
            prompt,
            execution,
            resolver,
            models,
            input: String::new(),
            tools: ToolSet::default(),
            store: StoreHandle::memory(),
            gateway: None,
            cancellation: Cancellation::new(),
            observer: Arc::new(NullObserver),
            capture: None,
            limits: RunLimits::default(),
        }
    }
    /// Sets input.
    #[must_use]
    pub fn with_input(mut self, v: impl Into<String>) -> Self {
        self.input = v.into();
        self
    }
    /// Sets tools.
    #[must_use]
    pub fn with_tools(mut self, v: ToolSet) -> Self {
        self.tools = v;
        self
    }
    /// Sets store.
    #[must_use]
    pub fn with_store(mut self, v: StoreHandle) -> Self {
        self.store = v;
        self
    }
    /// Sets gateway.
    #[must_use]
    pub fn with_gateway(mut self, v: GatewayClient) -> Self {
        self.gateway = Some(v);
        self
    }
    /// Sets cancellation.
    #[must_use]
    pub fn with_cancellation(mut self, v: Cancellation) -> Self {
        self.cancellation = v;
        self
    }
    /// Sets observer.
    #[must_use]
    pub fn with_observer(mut self, v: Arc<dyn Observer>) -> Self {
        self.observer = v;
        self
    }
    /// Sets sensitive capture.
    #[must_use]
    pub fn with_capture(mut self, v: Arc<dyn SensitiveCapture>) -> Self {
        self.capture = Some(v);
        self
    }
    /// Sets limits.
    #[must_use]
    pub fn with_limits(mut self, v: RunLimits) -> Self {
        self.limits = v;
        self
    }
}

/// Successful run output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput(String);
impl RunOutput {
    /// Borrows output text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.0
    }
    /// Consumes output text.
    #[must_use]
    pub fn into_text(self) -> String {
        self.0
    }
}

/// Executes one prompt.
///
/// # Errors
/// Returns a run error for invalid bindings, cancellation, model, tool, Lua, or store failures.
pub async fn run(request: RunRequest) -> Result<RunOutput, RunError> {
    let RunRequest {
        prompt,
        execution,
        resolver,
        models,
        input,
        tools,
        store,
        gateway,
        cancellation,
        observer,
        capture,
        limits: _,
    } = request;
    let legacy_models = crate::model::ModelCatalog::new(models.models.iter().map(|m| {
        crate::model::ModelDescriptor::new(
            crate::model::ModelId::gateway(m.id.as_str()),
            m.description.to_string(),
            m.context_tokens.get(),
            match m.thinking {
                ThinkingMode::Never => crate::model::ThinkingMode::Never,
                ThinkingMode::Always => crate::model::ThinkingMode::Always,
                ThinkingMode::Switchable => crate::model::ThinkingMode::Switchable,
            },
        )
        .with_dialect(match m.tool_protocol {
            ToolProtocol::OpenAi => crate::dialects::ToolDialectId::OpenAi,
            ToolProtocol::Gemma3ToolCode => crate::dialects::ToolDialectId::Gemma3ToolCode,
        })
    }));
    let legacy_tools: Vec<Arc<dyn crate::tools::Tool>> = tools
        .tools
        .iter()
        .cloned()
        .map(|tool| {
            Arc::new(LegacyTool {
                tool,
                cancellation: cancellation.clone(),
            }) as Arc<dyn crate::tools::Tool>
        })
        .collect();
    let legacy_store = crate::store::StoreRef::new(Box::new(LegacyStore(store)));
    let legacy_observer = LegacyObserver {
        inner: observer,
        execution: execution.clone(),
    };
    let legacy_capture = capture.map(|inner| LegacyCapture {
        inner,
        execution: execution.clone(),
    });
    let options = crate::execute::RunOptions {
        execution: &execution.0,
        observer: &legacy_observer,
        client: gateway.map(|g| g.legacy()),
        debug: legacy_capture
            .as_ref()
            .map(|c| c as &dyn crate::debug::DebugCapture),
    };
    let resolution = crate::execute::ResolutionContext {
        picker: &resolver.0,
        models: &legacy_models,
    };
    crate::cancel::scope(
        cancellation.0,
        crate::execute::run(
            &prompt.0,
            &input,
            resolution,
            &legacy_tools,
            &legacy_store,
            options,
        ),
    )
    .await
    .map(RunOutput)
    .map_err(RunError::from)
}

struct LegacyTool {
    tool: Arc<dyn Tool>,
    cancellation: Cancellation,
}
#[async_trait::async_trait]
impl crate::tools::Tool for LegacyTool {
    fn id(&self) -> crate::tools::ToolId {
        crate::tools::ToolId::new(self.tool.spec().id().server(), self.tool.spec().id().name())
    }
    fn wire_name(&self) -> &str {
        self.tool.spec().wire_name()
    }
    fn description(&self) -> &str {
        self.tool.spec().description()
    }
    fn parameters_schema(&self) -> serde_json::Value {
        self.tool.spec().parameters().value()
    }
    async fn call(&self, args: serde_json::Value) -> crate::Result<String> {
        let object = JsonObject(Json::from_value(&args));
        self.tool
            .call(ToolRequest {
                arguments: &object,
                cancellation: &self.cancellation,
            })
            .await
            .map(|o| o.text)
            .map_err(|e| crate::Error::Lua(e.to_string()))
    }
    fn untrusted_output(&self) -> bool {
        true
    }
}

struct LegacyStore(StoreHandle);
impl crate::store::Store for LegacyStore {
    fn write(&mut self, p: &str, c: &str) -> Result<(), crate::store::StoreError> {
        legacy_path(p).and_then(|p| {
            self.0
                .write(&p, c)
                .map_err(|_| legacy_store_error(p.as_str()))
        })
    }
    fn append(&mut self, p: &str, c: &str) -> Result<(), crate::store::StoreError> {
        legacy_path(p).and_then(|p| {
            self.0
                .append(&p, c)
                .map_err(|_| legacy_store_error(p.as_str()))
        })
    }
    fn read_lines(&self, p: &str) -> Result<String, crate::store::StoreError> {
        legacy_path(p).and_then(|p| {
            self.0
                .read_lines(&p)
                .map_err(|_| legacy_store_error(p.as_str()))
        })
    }
    fn read(&self, p: &str) -> Result<String, crate::store::StoreError> {
        legacy_path(p).and_then(|p| self.0.read(&p).map_err(|_| legacy_store_error(p.as_str())))
    }
    fn str_replace(&mut self, p: &str, o: &str, n: &str) -> Result<(), crate::store::StoreError> {
        legacy_path(p).and_then(|p| {
            self.0
                .replace(&p, o, n)
                .map_err(|_| legacy_store_error(p.as_str()))
        })
    }
    fn delete(&mut self, p: &str) -> Result<(), crate::store::StoreError> {
        legacy_path(p).and_then(|p| {
            self.0
                .delete(&p)
                .map_err(|_| legacy_store_error(p.as_str()))
        })
    }
    fn glob(&self, p: &str) -> Result<Vec<String>, crate::store::StoreError> {
        let pattern = GlobPattern::try_from(p).map_err(|_| legacy_store_error(p))?;
        self.0
            .glob(&pattern)
            .map(|v| v.into_iter().map(|p| p.0.into()).collect())
            .map_err(|_| legacy_store_error(p))
    }
    fn exists(&self, p: &str) -> bool {
        StorePath::try_from(p)
            .ok()
            .and_then(|p| self.0.exists(&p).ok())
            .unwrap_or(false)
    }
}
fn legacy_path(p: &str) -> Result<StorePath, crate::store::StoreError> {
    StorePath::try_from(p).map_err(|_| legacy_store_error(p))
}
fn legacy_store_error(p: &str) -> crate::store::StoreError {
    crate::store::StoreError::NotFound { path: p.to_owned() }
}

/// Operational category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Operation {
    /// Whole run.
    Run,
    /// Section.
    Section,
    /// Model turn.
    ModelTurn,
    /// Tool call.
    ToolCall,
    /// Lua compile.
    LuaCompile,
    /// Lua load.
    LuaLoad,
    /// Lua block.
    LuaBlock,
    /// Store.
    Store,
    /// Fanout arm.
    FanoutArm,
}
/// Lifecycle outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Outcome {
    /// Started.
    Started,
    /// Succeeded.
    Succeeded,
    /// Failed.
    Failed,
    /// Cancelled.
    Cancelled,
    /// Exhausted.
    Exhausted,
    /// Truncated.
    Truncated,
}
/// Payload-free operational event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// Lifecycle transition.
    Lifecycle {
        operation: Operation,
        outcome: Outcome,
    },
    /// Bounded author checkpoint.
    Checkpoint { message: Box<str> },
}
/// Borrowed observation.
#[derive(Debug, Clone)]
pub struct Observation<'a> {
    execution: &'a ExecutionId,
    section: Option<&'a str>,
    event: &'a Event,
}
impl Observation<'_> {
    /// Execution id.
    #[must_use]
    pub fn execution(&self) -> &ExecutionId {
        self.execution
    }
    /// Optional section heading.
    #[must_use]
    pub fn section(&self) -> Option<&str> {
        self.section
    }
    /// Event.
    #[must_use]
    pub fn event(&self) -> &Event {
        self.event
    }
}
/// Operational observer.
pub trait Observer: Send + Sync {
    /// Receives one payload-free observation.
    fn observe(&self, observation: Observation<'_>);
}
/// Observer that discards events.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullObserver;
impl Observer for NullObserver {
    fn observe(&self, _: Observation<'_>) {}
}
struct LegacyObserver {
    inner: Arc<dyn Observer>,
    execution: ExecutionId,
}
impl crate::observe::Observer for LegacyObserver {
    fn observe(&self, _: &str, section: &str, detail: &str) {
        let outcome = if detail.contains("started") || detail.contains("closing") {
            Outcome::Started
        } else if detail.contains("failed") {
            Outcome::Failed
        } else if detail.contains("truncated") {
            Outcome::Truncated
        } else {
            Outcome::Succeeded
        };
        let operation = if detail.starts_with("Run") {
            Operation::Run
        } else if detail.starts_with("Section") {
            Operation::Section
        } else if detail.starts_with("Model") {
            Operation::ModelTurn
        } else if detail.starts_with("Tool call") {
            Operation::ToolCall
        } else if detail.starts_with("Fanout") {
            Operation::FanoutArm
        } else {
            Operation::LuaBlock
        };
        let event = Event::Lifecycle { operation, outcome };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner.observe(Observation {
                execution: &self.execution,
                section: Some(section),
                event: &event,
            });
        }));
    }
}

/// Explicitly sensitive model exchange.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SensitiveModelEvent {
    /// Request body.
    Request { body: Json },
    /// Response body and metadata.
    Response {
        body: Json,
        finish_reason: Option<Box<str>>,
        reasoning_content: Option<Box<str>>,
    },
}
/// Opt-in secret-bearing capture.
pub trait SensitiveCapture: Send + Sync {
    /// Receives one sensitive model event and must return promptly.
    fn capture(
        &self,
        execution: &ExecutionId,
        section: Option<&str>,
        turn: NonZeroU64,
        event: SensitiveModelEvent,
    );
}
struct LegacyCapture {
    inner: Arc<dyn SensitiveCapture>,
    execution: ExecutionId,
}
impl crate::debug::DebugCapture for LegacyCapture {
    fn on_event(&self, _: &str, section: &str, turn: u32, event: crate::debug::DebugEvent) {
        let Some(turn) = NonZeroU64::new(u64::from(turn)) else {
            return;
        };
        let event = match event {
            crate::debug::DebugEvent::Request { body } => SensitiveModelEvent::Request {
                body: Json::from_value(&body),
            },
            crate::debug::DebugEvent::Response {
                body,
                finish_reason,
                reasoning_content,
            } => SensitiveModelEvent::Response {
                body: Json::from_value(&body),
                finish_reason: finish_reason.map(Into::into),
                reasoning_content: reasoning_content.map(Into::into),
            },
        };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner
                .capture(&self.execution, Some(section), turn, event);
        }));
    }
}

macro_rules! opaque_error {
    ($name:ident,$text:literal) => {
        #[doc=$text]
        #[derive(Debug, thiserror::Error)]
        #[error($text)]
        pub struct $name;
    };
}
opaque_error!(ToolCallError, "tool call");
opaque_error!(ParseError, "prompt parse");
opaque_error!(RunError, "prompt run");
impl ToolCallError {
    /// Creates a tool-call failure without retaining sensitive text.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}
impl ParseError {
    fn message(_message: &str) -> Self {
        Self
    }
}
impl From<crate::Error> for ParseError {
    fn from(_: crate::Error) -> Self {
        Self
    }
}
impl From<crate::Error> for RunError {
    fn from(_: crate::Error) -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_values_are_send_sync() {
        fn assert<T: Send + Sync>() {}
        assert::<Cancellation>();
        assert::<GatewayClient>();
        assert::<StoreHandle>();
        assert::<ToolSet>();
    }
    #[test]
    fn validates_json_and_catalogs() {
        assert!("{}".parse::<JsonObject>().is_ok());
        assert!("[]".parse::<JsonObject>().is_err());
        let id = "m".parse().expect("id");
        let m = ModelDescriptor::try_new(
            id,
            "model",
            NonZeroU32::new(1).expect("nonzero"),
            ThinkingMode::Never,
            ToolProtocol::OpenAi,
        )
        .expect("model");
        assert_eq!(ModelCatalog::try_new([m]).expect("catalog").len(), 1);
    }
    #[test]
    fn parser_facade_is_immutable() {
        let source = "---\nname: n\ndescription: d\npromptforge: 1\n---\n# T\n## S\ntext";
        let p = Prompt::parse(source).expect("parse");
        assert_eq!(p.name(), "n");
        assert_eq!(p.sections().next().expect("section").heading(), "S");
    }
    #[test]
    fn redacts_gateway_credential() {
        let credential = GatewayCredential::try_new("secret").expect("credential");
        assert!(!format!("{credential:?}").contains("secret"));
    }
}
