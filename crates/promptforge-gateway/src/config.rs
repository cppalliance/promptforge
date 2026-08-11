//! Gateway configuration: `gateway.toml` parsing, `${VAR}` interpolation, and
//! semantic validation.

use std::fmt;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::queue::QueueConfig;

mod imp;
mod interpolate;
mod validate;

#[cfg(test)]
pub(crate) use interpolate::interpolate;
pub(crate) use interpolate::interpolate_value;

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

/// A secret string (an API key or the shared token) that never serializes and
/// redacts in both `Debug` and `Display`.
///
/// The type has no public `Deserialize` or `From<String>` impl: it is
/// constructed only inside this crate (via a crate-private constructor and a
/// private field deserializer), so a redacting secret can never be minted or
/// round-tripped by a downstream consumer. `expose` is the single accessor.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// Wrap a plaintext secret. Crate-internal: the only construction path,
    /// used by config deserialization and by adapters that mint an ephemeral
    /// loopback credential.
    #[must_use]
    pub(crate) fn new(value: String) -> Secret {
        Secret(value)
    }

    /// The secret's bytes. The one place a secret is read, when building auth.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is empty (an intentionally credential-free endpoint).
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Protocol {
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
    pub(crate) server: ServerConfig,
    /// Waiting-queue settings for limited endpoints.
    pub(crate) queue: QueueConfig,
    /// Cache and binary settings for gateway-owned local inference.
    pub(crate) local: LocalConfig,
    /// Physical compute resources with concurrency limits.
    pub(crate) devices: Vec<DeviceConfig>,
    /// The configured backends.
    pub(crate) endpoints: Vec<EndpointConfig>,
    /// The routing table from model name to remote backend.
    pub(crate) models: Vec<ModelConfig>,
    /// Local generative models served by a managed `llama-server` child.
    pub(crate) local_models: Vec<LocalModelConfig>,
    /// Optional built-in tool configuration. Absent when no `[tools]` section
    /// is present.
    pub(crate) tools: Option<ToolsConfig>,
}

/// Private deserialization DTO for [`Config`]. Holds the raw TOML shape before
/// validation; never exposed publicly, so no serde impl reaches the API.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    server: ServerConfig,
    #[serde(default)]
    queue: QueueConfig,
    #[serde(default)]
    local: LocalConfig,
    #[serde(rename = "device", default)]
    devices: Vec<DeviceConfig>,
    #[serde(rename = "endpoint", default)]
    endpoints: Vec<EndpointConfig>,
    #[serde(rename = "model", default)]
    models: Vec<ModelConfig>,
    #[serde(rename = "local_model", default)]
    local_models: Vec<LocalModelConfig>,
    #[serde(default)]
    tools: Option<ToolsConfig>,
}

impl From<RawConfig> for Config {
    fn from(raw: RawConfig) -> Config {
        Config {
            server: raw.server,
            queue: raw.queue,
            local: raw.local,
            devices: raw.devices,
            endpoints: raw.endpoints,
            models: raw.models,
            local_models: raw.local_models,
            tools: raw.tools,
        }
    }
}

/// Whether a device is a remote provider or a local GPU managed by the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub(crate) enum DeviceKind {
    /// A remote HTTP provider; concurrency is flat (no lanes required).
    Remote,
    /// A local GPU; concurrency comes from `[[device.lane]]` entries.
    Local,
}

/// One compute device declared as `[[device]]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct DeviceConfig {
    /// Operator-chosen device id, referenced by endpoints and local models.
    pub id: String,
    /// Remote provider or local GPU.
    #[serde(rename = "type")]
    pub kind: DeviceKind,
    /// Max concurrent requests for a remote device. Ignored for local devices
    /// (use lanes instead).
    #[serde(default)]
    pub concurrency: Option<usize>,
    /// Lanes nested via `[[device.lane]]` under this device.
    #[serde(default, rename = "lane")]
    pub lanes: Vec<LaneConfig>,
}

/// A concurrency lane within a local device (`[[device.lane]]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct LaneConfig {
    /// Lane id referenced by `[[local_model]].lane`.
    pub id: String,
    /// Max concurrent inferences on this lane.
    pub concurrency: usize,
    /// Optional explicit device id (redundant when nested under `[[device]]`).
    #[serde(default)]
    pub device: Option<String>,
}

/// Settings under `[local]` for artifact cache paths.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct LocalConfig {
    /// Root directory for GGUF files and the pinned `llama-server` install.
    ///
    /// Defaults to `~/.promptforge` (Windows: `%USERPROFILE%\.promptforge`).
    /// Models land in `<cache_dir>/models`; llama.cpp installs in
    /// `<cache_dir>/llama.cpp`.
    #[serde(default)]
    pub cache_dir: Option<String>,
}

/// One local generative model declared as `[[local_model]]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct LocalModelConfig {
    /// Caller-facing model name in `/v1/models` and chat completions.
    pub name: String,
    /// Prose describing the model for catalog consumers and semantic bind.
    pub description: String,
    /// Hugging Face (or other) URL, or a local filesystem path to a GGUF.
    pub source: String,
    /// Optional SHA-256 pin (lowercase hex). Verified after download when set.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Optional device id (`[[device]]`); used with [`Self::lane`] for concurrency.
    #[serde(default)]
    pub device: Option<String>,
    /// Optional lane id under the device (`[[device.lane]]`).
    #[serde(default)]
    pub lane: Option<String>,
    /// Context window size in tokens (`--ctx-size`).
    pub context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    #[serde(default)]
    pub thinking: ThinkingMode,
    /// GPU layers offloaded (`-ngl`). Defaults to 99.
    #[serde(default = "default_gpu_layers")]
    pub gpu_layers: u32,
    /// Enable flash attention (`--flash-attn on`). Defaults to true.
    #[serde(default = "default_true")]
    pub flash_attention: bool,
    /// KV cache type for K. Defaults to `q8_0`.
    #[serde(default = "default_cache_type_k")]
    pub cache_type_k: String,
    /// KV cache type for V. Defaults to `q4_0`.
    #[serde(default = "default_cache_type_v")]
    pub cache_type_v: String,
    /// Generation ceiling (`--n-predict`). Defaults to 8192.
    #[serde(default = "default_n_predict")]
    pub n_predict: u32,
    /// Optional path to a Jinja chat template file (`--chat-template-file`).
    ///
    /// Use when the GGUF embeds a template without tool-calling support (common
    /// for Mistral Small Instruct quants) and a tools-capable override is needed.
    #[serde(default)]
    pub chat_template_file: Option<String>,
}

/// Server-level settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct ServerConfig {
    /// The socket address to bind.
    pub bind: SocketAddr,
    /// The shared bearer key every `/v1/*` request must present.
    #[serde(deserialize_with = "de_secret")]
    pub key: Secret,
}

/// One backend the gateway can forward to.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct EndpointConfig {
    /// The endpoint's id: an operator-chosen handle referenced by `[[model]]`
    /// entries. Distinct from a model's caller-facing `name`.
    pub id: String,
    /// The wire protocol this endpoint speaks.
    pub protocol: Protocol,
    /// The backend base URL (a trailing slash is trimmed).
    pub base_url: String,
    /// The credential sent to this backend.
    #[serde(deserialize_with = "de_secret")]
    pub api_key: Secret,
    /// Maximum in-flight requests to this endpoint. Absent means unlimited
    /// (the waiting queue is a no-op pass-through for that endpoint).
    /// When [`Self::device`] is set and this field is absent, the device's
    /// concurrency is used instead.
    #[serde(default)]
    pub concurrency: Option<usize>,
    /// Optional remote device id whose concurrency governs this endpoint.
    #[serde(default)]
    pub device: Option<String>,
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
pub(crate) enum ThinkingMode {
    /// The backend never emits thinking tokens; a per-call switch is ignored.
    #[default]
    Never,
    /// The backend always emits thinking tokens; a per-call switch is ignored.
    Always,
    /// The client may turn thinking on or off per request.
    Switchable,
}

/// One model name and the backend it resolves to.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct ModelConfig {
    /// The name callers request and that a slot resolves to.
    pub name: String,
    /// Prose describing the model for catalog consumers and semantic bind.
    pub description: String,
    /// Context window size in tokens.
    pub context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    #[serde(default)]
    pub thinking: ThinkingMode,
    /// The string the backend knows this model by.
    pub upstream: String,
    /// The endpoint ids serving this model (v0 uses the first).
    pub endpoints: Vec<String>,
    /// A `max_tokens` default supplied when the caller omits one.
    #[serde(default)]
    pub default_max_tokens: Option<u32>,
}

/// Built-in tool configuration under the `[tools]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct ToolsConfig {
    /// The web-search tool configuration. Absent when no `[tools.web_search]`
    /// section is present.
    #[serde(default)]
    pub web_search: Option<WebSearchConfig>,
}

/// Configuration for the web-search tool.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct WebSearchConfig {
    /// The search provider backing the tool.
    pub provider: SearchProvider,
    /// The credential sent to the search provider.
    #[serde(deserialize_with = "de_secret")]
    pub api_key: Secret,
    /// The search API base URL. Defaults to the Brave Search endpoint;
    /// override to point at a proxy or a test server.
    #[serde(default = "default_brave_base_url")]
    pub base_url: String,
    /// Used when the request omits `count`.
    #[serde(default = "default_web_search_count")]
    pub default_count: u8,
    /// Clamp and over-fetch ceiling for result counts.
    #[serde(default = "default_web_search_max_count")]
    pub max_count: u8,
    /// Diversity cap per hostname group.
    #[serde(default = "default_web_search_max_per_host")]
    pub max_per_host: u8,
    /// Applied when the request omits `freshness` and this is non-empty.
    #[serde(default)]
    pub default_freshness: String,
    /// Applied when the request omits `safesearch` and this is non-empty.
    #[serde(default)]
    pub default_safesearch: String,
    /// When true, scrub known tracking query params from result URLs.
    #[serde(default = "default_true")]
    pub strip_tracking: bool,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub(crate) enum SearchProvider {
    /// The Brave Search API.
    Brave,
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}
