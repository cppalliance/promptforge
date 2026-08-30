//! Gateway configuration: `gateway.toml` parsing, `${VAR}` interpolation, and
//! semantic validation.

use std::fmt;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

mod accessors;
mod companion;
mod imp;
mod interpolate;
mod validate;
mod workshop;

pub use companion::{
    DraftTokenMax, DraftTokenMaxError, MultimodalProjectorConfig, SpeculationType,
    SpeculativeConfig,
};
#[cfg(test)]
pub(crate) use interpolate::interpolate;
pub(crate) use interpolate::interpolate_value;
pub use workshop::{WorkshopConfig, WorkshopTapeConfig, WorkshopVoiceConfig};

#[cfg(test)]
use crate::error::ConfigError;

#[cfg(test)]
mod tests;

fn default_gpu_layers() -> u32 {
    99
}

fn default_true() -> bool {
    true
}

fn default_cache_type_k() -> String {
    "q8_0".to_owned()
}

fn default_cache_type_v() -> String {
    "q4_0".to_owned()
}

fn default_n_predict() -> u32 {
    8192
}

fn default_parallel() -> u32 {
    1
}

fn default_max_queue() -> usize {
    100
}

/// A secret string (an API key or the shared token) that never serializes and
/// redacts in both `Debug` and `Display`.
///
/// The type has no public `Deserialize` or `From<String>` impl: configuration
/// deserialization constructs it through a private field deserializer, so a
/// redacting secret can never be round-tripped from a downstream consumer.
/// `expose` is the single read accessor.
#[derive(Clone)]
#[non_exhaustive]
pub struct Secret(String);

impl Secret {
    /// Wrap a plaintext secret.
    ///
    /// Used by config deserialization and by the gateway's adapters that mint
    /// an ephemeral loopback credential.
    #[must_use]
    pub fn new(value: String) -> Secret {
        Secret(value)
    }

    /// The secret's bytes. The one place a secret is read, when building auth.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is empty (an intentionally credential-free endpoint).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Deserialize a [`Secret`] field from a bare TOML string without exposing a
/// public `Deserialize` impl on the redacting type.
fn de_secret<'de, D>(deserializer: D) -> Result<Secret, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    Ok(Secret::new(raw))
}

/// Serialize a [`Secret`] field as `"***"`: a serialized configuration never
/// carries credential material, and a reader treats the marker as "keep the
/// existing value" on write.
pub(crate) fn ser_redacted<S>(_: &Secret, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str("***")
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(redacted)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("redacted")
    }
}

/// The wire protocol an endpoint speaks. v0 supports only the OpenAI shape;
/// the Anthropic translation shim is deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Protocol {
    /// The OpenAI `/chat/completions` shape.
    Openai,
}

/// The whole gateway configuration.
///
/// Validated on construction: a value of this type cannot hold an invalid
/// configuration. Deserialization goes through a private raw DTO and a
/// validating conversion, so `Config` itself carries no public `Deserialize`
/// impl and cannot be built from arbitrary TOML without validation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Config {
    /// Server bind address and shared key.
    server: ServerConfig,
    /// Cache and binary settings for gateway-owned local inference.
    local: LocalConfig,
    /// Named pools of compute with a shared concurrency limit and queue.
    dominions: Vec<DominionConfig>,
    /// The configured backends.
    endpoints: Vec<EndpointConfig>,
    /// The routing table from model name to remote backend.
    models: Vec<ModelConfig>,
    /// Local generative models served by a managed `llama-server` child.
    local_models: Vec<LocalModelConfig>,
    /// The profile's top-level `models = [...]` allowlist: the catalog subset
    /// the profile selected. `None` loads the full catalog.
    model_allowlist: Option<Vec<String>>,
    /// Optional built-in tool configuration. Absent when no `[tools]` section
    /// is present.
    tools: Option<ToolsConfig>,
    /// Optional hosted-workshop configuration. Absent when no `[workshop]`
    /// section is present. Boot-only, like `[server]`.
    workshop: Option<WorkshopConfig>,
    /// Which file each merged entry came from, recorded during include
    /// resolution. Empty when the config was built without it (for example
    /// [`Config::from_toml_str`]).
    provenance: crate::profile::Provenance,
}

/// Private DTO for [`Config`]. Holds the raw TOML shape before validation and
/// is never exposed publicly, so no serde impl reaches the API. `Serialize`
/// lets the gateway render the running config as JSON through this shape,
/// with `Secret` fields redacted.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfig {
    server: ServerConfig,
    #[serde(default)]
    local: LocalConfig,
    #[serde(rename = "dominion", default)]
    dominions: Vec<DominionConfig>,
    #[serde(rename = "endpoint", default)]
    endpoints: Vec<EndpointConfig>,
    #[serde(rename = "model", default)]
    models: Vec<ModelConfig>,
    #[serde(rename = "local_model", default)]
    local_models: Vec<LocalModelConfig>,
    /// The top-level `models = [...]` allowlist: an array of model names,
    /// distinct from the `[[model]]` definition array mapped to `models`.
    #[serde(rename = "models", default)]
    model_allowlist: Option<Vec<String>>,
    #[serde(default)]
    tools: Option<ToolsConfig>,
    #[serde(default)]
    workshop: Option<WorkshopConfig>,
}

impl From<RawConfig> for Config {
    fn from(raw: RawConfig) -> Config {
        Config {
            server: raw.server,
            local: raw.local,
            dominions: raw.dominions,
            endpoints: raw.endpoints,
            models: raw.models,
            local_models: raw.local_models,
            model_allowlist: raw.model_allowlist,
            tools: raw.tools,
            workshop: raw.workshop,
            provenance: crate::profile::Provenance::default(),
        }
    }
}

/// Whether a dominion pools remote providers or local GPUs managed by the
/// gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum DominionKind {
    /// A pool of remote HTTP providers, bindable by `[[endpoint]]` entries.
    Remote,
    /// A local GPU, bindable by `[[local_model]]` entries.
    Local,
}

/// What a dominion's admission queue does when it is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum QueuePolicy {
    /// Wait for a slot up to `max_queue` waiting requests, then reject.
    #[default]
    Queue,
    /// Reject immediately when no concurrency slot is free (fail-fast).
    Reject,
}

/// One named pool of compute declared as `[[dominion]]`.
///
/// A dominion carries a concurrency limit and a bounded waiting queue that
/// every bound endpoint or local model shares. An endpoint or local model
/// without a `dominion` is unlimited, as when no cap is set at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct DominionConfig {
    /// Operator-chosen dominion id, referenced by endpoints and local models.
    id: String,
    /// Remote provider pool or local GPU.
    kind: DominionKind,
    /// Max concurrent requests admitted across every binder. Absent means
    /// unlimited.
    #[serde(default)]
    max_concurrency: Option<usize>,
    /// Max waiting requests before new admits are rejected. Defaults to 100.
    #[serde(default = "default_max_queue")]
    max_queue: usize,
    /// Whether a full queue waits or rejects. Defaults to `queue`.
    #[serde(default)]
    policy: QueuePolicy,
    /// Whether waiting callers are served round-robin by client key.
    /// Defaults to true.
    #[serde(default = "default_true")]
    fair_scheduling: bool,
    /// VRAM budget in gibibytes for co-residency checks. Local kind only.
    #[serde(default)]
    vram_gb: Option<u32>,
}

/// Settings under `[local]` for artifact cache paths.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LocalConfig {
    /// Root directory for GGUF files and the pinned `llama-server` install.
    ///
    /// Defaults to `~/.promptforge` (Windows: `%USERPROFILE%\.promptforge`).
    /// Models land in `<cache_dir>/models`; llama.cpp installs in
    /// `<cache_dir>/llama.cpp`.
    #[serde(default)]
    cache_dir: Option<String>,
}

/// One local generative model declared as `[[local_model]]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LocalModelConfig {
    /// Caller-facing model name in `/v1/models` and chat completions.
    name: String,
    /// The workload this model serves. Defaults to `chat`.
    #[serde(default)]
    kind: ModelKind,
    /// Prose describing the model for catalog consumers and semantic bind.
    description: String,
    /// Hugging Face (or other) URL, or a local filesystem path to a GGUF.
    source: String,
    /// Optional SHA-256 pin (lowercase hex). Verified after download when set.
    #[serde(default)]
    sha256: Option<String>,
    /// Optional local dominion id (`[[dominion]]`) binding this model.
    #[serde(default)]
    dominion: Option<String>,
    /// Max concurrent inferences: the child's `--parallel` and, when no
    /// dominion is bound, the model's gateway queue limit. Defaults to 1.
    #[serde(default = "default_parallel")]
    parallel: u32,
    /// VRAM footprint estimate in gibibytes for the dominion co-residency
    /// check.
    #[serde(default)]
    vram_gb: Option<u32>,
    /// Context window size in tokens (`--ctx-size`).
    context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    #[serde(default)]
    thinking: ThinkingMode,
    /// GPU layers offloaded (`-ngl`). Defaults to 99.
    #[serde(default = "default_gpu_layers")]
    gpu_layers: u32,
    /// Enable flash attention (`--flash-attn on`). Defaults to true.
    #[serde(default = "default_true")]
    flash_attention: bool,
    /// KV cache type for K. Defaults to `q8_0`.
    #[serde(default = "default_cache_type_k")]
    cache_type_k: String,
    /// KV cache type for V. Defaults to `q4_0`.
    #[serde(default = "default_cache_type_v")]
    cache_type_v: String,
    /// Generation ceiling (`--n-predict`). Defaults to 8192.
    #[serde(default = "default_n_predict")]
    n_predict: u32,
    /// Optional path to a Jinja chat template file (`--chat-template-file`).
    ///
    /// Use when the GGUF embeds a template without tool-calling support (common
    /// for Mistral Small Instruct quants) and a tools-capable override is needed.
    #[serde(default)]
    chat_template_file: Option<String>,
    /// Optional speculative-decoding drafter companion
    /// (`[local_model.speculative]`). Chat kind only.
    #[serde(default)]
    speculative: Option<SpeculativeConfig>,
    /// Optional multimodal projector companion
    /// (`[local_model.multimodal_projector]`). Chat kind only.
    #[serde(default)]
    multimodal_projector: Option<MultimodalProjectorConfig>,
    /// Capability metadata advertised on the catalog.
    #[serde(default, flatten)]
    capabilities: Capabilities,
}

/// Server-level settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ServerConfig {
    /// The socket address to bind.
    bind: SocketAddr,
    /// The shared bearer key every `/v1/*` request must present.
    #[serde(deserialize_with = "de_secret", serialize_with = "ser_redacted")]
    api_key: Secret,
}

/// One backend the gateway can forward to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct EndpointConfig {
    /// The endpoint's id: an operator-chosen handle referenced by `[[model]]`
    /// entries. Distinct from a model's caller-facing `name`.
    id: String,
    /// The wire protocol this endpoint speaks.
    protocol: Protocol,
    /// The backend base URL (a trailing slash is trimmed).
    base_url: String,
    /// The credential sent to this backend.
    #[serde(deserialize_with = "de_secret", serialize_with = "ser_redacted")]
    api_key: Secret,
    /// Optional remote dominion id (`[[dominion]]`) whose shared limit and
    /// queue govern this endpoint. Absent means unlimited pass-through.
    #[serde(default)]
    dominion: Option<String>,
}

/// How a model exposes chain-of-thought / thinking tokens to callers.
///
/// Catalogued on each `[[model]]` so hosts can filter bindings before a
/// request is built. `never` and `always` mean the backend ignores a
/// per-call switch; `switchable` means the client may emit
/// `chat_template_kwargs.enable_thinking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ThinkingMode {
    /// The backend never emits thinking tokens; a per-call switch is ignored.
    #[default]
    Never,
    /// The backend always emits thinking tokens; a per-call switch is ignored.
    Always,
    /// The client may turn thinking on or off per request.
    Switchable,
}

/// The tool-calling dialect a chat model speaks.
///
/// `openai` (the default) forwards tool definitions verbatim and expects
/// native wire `tool_calls`. `gemma3_tool_code` emulates tool calling for
/// backends without a native tool array: the gateway injects a tool guide
/// into the system prompt, strips `tools`/`tool_choice` from the outgoing
/// request, and parses `tool_code` content fences from the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolDialect {
    /// Native OpenAI tool calling. The default.
    #[default]
    Openai,
    /// Emulated Gemma3 `tool_code` content-fence protocol.
    Gemma3ToolCode,
}

impl fmt::Display for ToolDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spelling = match self {
            ToolDialect::Openai => "openai",
            ToolDialect::Gemma3ToolCode => "gemma3_tool_code",
        };
        f.write_str(spelling)
    }
}

/// The workload a model serves: chat completions, embeddings, or
/// classification.
///
/// The kind scopes which configuration fields are meaningful: chat-only
/// fields (for example `thinking`, `default_max_tokens`,
/// `chat_template_file`) are rejected for non-chat kinds at validation,
/// while `context` applies to every kind. The catalog carries the kind so
/// clients can filter before building a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ModelKind {
    /// Chat completions (`POST /v1/chat/completions`). The default.
    #[default]
    Chat,
    /// Text embeddings.
    Embedding,
    /// Classification / reranking.
    Classifier,
}

impl fmt::Display for ModelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let spelling = match self {
            ModelKind::Chat => "chat",
            ModelKind::Embedding => "embedding",
            ModelKind::Classifier => "classifier",
        };
        f.write_str(spelling)
    }
}

/// Capability metadata advertised on the model catalog.
///
/// These fields describe what a model can do rather than how the gateway
/// reaches it. They are flattened into `[[model]]` and `[[local_model]]`,
/// validated at load, and surfaced verbatim on `GET /v1/models` so clients
/// can shape requests before sending them. The effort knobs are chat-only
/// and require a `thinking` mode other than `never`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Capabilities {
    /// Max output tokens the model can emit per completion. Must not exceed
    /// `context` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_output: Option<u32>,
    /// Sampling temperature applied when the caller omits one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_temperature: Option<f32>,
    /// Whether the model accepts image inputs. Defaults to false; a
    /// `[local_model.multimodal_projector]` companion implies true.
    #[serde(default)]
    images: bool,
    /// Whether the model can emit parallel tool calls. Defaults to false.
    #[serde(default)]
    parallel_tool_calls: bool,
    /// The reasoning-effort levels the model accepts. Empty means the model
    /// has no effort knob.
    #[serde(default)]
    effort_levels: Vec<String>,
    /// The effort level applied when the caller omits one; requires a
    /// non-empty `effort_levels` and must name a listed level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_effort: Option<String>,
    /// Whether the model adaptively chooses how much to think per request;
    /// chat kind only. Defaults to false.
    #[serde(default)]
    adaptive_thinking: bool,
}

/// One model name and the backend it resolves to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ModelConfig {
    /// The name callers request and that a slot resolves to.
    name: String,
    /// The workload this model serves. Defaults to `chat`.
    #[serde(default)]
    kind: ModelKind,
    /// Prose describing the model for catalog consumers and semantic bind.
    description: String,
    /// Context window size in tokens.
    context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    #[serde(default)]
    thinking: ThinkingMode,
    /// The string the backend knows this model by.
    upstream: String,
    /// The endpoint ids serving this model (v0 uses the first).
    endpoints: Vec<String>,
    /// A `max_tokens` default supplied when the caller omits one.
    #[serde(default)]
    default_max_tokens: Option<u32>,
    /// The tool-calling dialect this model speaks. Defaults to `openai`.
    #[serde(default)]
    tool_dialect: ToolDialect,
    /// Capability metadata advertised on the catalog.
    #[serde(default, flatten)]
    capabilities: Capabilities,
}

/// Built-in tool configuration under the `[tools]` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToolsConfig {
    /// The web-search tool configuration. Absent when no `[tools.web_search]`
    /// section is present.
    #[serde(default)]
    web_search: Option<WebSearchConfig>,
}

/// Configuration for the web-search tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WebSearchConfig {
    /// The search provider backing the tool.
    provider: SearchProvider,
    /// The credential sent to the search provider.
    #[serde(deserialize_with = "de_secret", serialize_with = "ser_redacted")]
    api_key: Secret,
    /// The search API base URL. Defaults to the Brave Search endpoint;
    /// override to point at a proxy or a test server.
    #[serde(default = "default_brave_base_url")]
    base_url: String,
    /// Used when the request omits `count`.
    #[serde(default = "default_web_search_count")]
    default_count: u8,
    /// Clamp and over-fetch ceiling for result counts.
    #[serde(default = "default_web_search_max_count")]
    max_count: u8,
    /// Diversity cap per hostname group.
    #[serde(default = "default_web_search_max_per_host")]
    max_per_host: u8,
    /// Applied when the request omits `freshness` and this is non-empty.
    #[serde(default)]
    default_freshness: String,
    /// Applied when the request omits `safesearch` and this is non-empty.
    #[serde(default)]
    default_safesearch: String,
    /// When true, scrub known tracking query params from result URLs.
    #[serde(default = "default_true")]
    strip_tracking: bool,
}

/// The default Brave Search API base URL.
fn default_brave_base_url() -> String {
    "https://api.search.brave.com/res/v1".to_string()
}

fn default_web_search_count() -> u8 {
    10
}

fn default_web_search_max_count() -> u8 {
    20
}

fn default_web_search_max_per_host() -> u8 {
    2
}

/// A web-search provider. v0 supports only Brave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SearchProvider {
    /// The Brave Search API.
    Brave,
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}
