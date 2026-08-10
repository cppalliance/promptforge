//! Gateway configuration: `gateway.toml` parsing, `${VAR}` interpolation, and
//! semantic validation.

use std::collections::HashSet;
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::queue::QueueConfig;

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
#[derive(Clone, Deserialize)]
#[serde(from = "String")]
pub struct Secret(String);

impl Secret {
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

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Secret(value)
    }
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
/// configuration. Deserialization goes through the private [`RawConfig`] DTO and
/// a validating conversion, so `Config` itself carries no public `Deserialize`
/// impl and cannot be built from arbitrary TOML without validation.
#[derive(Debug)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Default, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct ServerConfig {
    /// The socket address to bind.
    pub bind: SocketAddr,
    /// The shared bearer key every `/v1/*` request must present.
    pub key: Secret,
}

/// One backend the gateway can forward to.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct ToolsConfig {
    /// The web-search tool configuration. Absent when no `[tools.web_search]`
    /// section is present.
    #[serde(default)]
    pub web_search: Option<WebSearchConfig>,
}

/// Configuration for the web-search tool.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct WebSearchConfig {
    /// The search provider backing the tool.
    pub provider: SearchProvider,
    /// The credential sent to the search provider.
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

impl Config {
    /// Load a configuration file with recursive `include` resolution.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) if the file cannot be read,
    /// an include cycle or depth limit is hit, an interpolation is malformed or
    /// references an unset variable, the TOML is invalid, or a semantic check
    /// fails.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_gateway::Config;
    /// use std::path::Path;
    ///
    /// # fn demo() -> Result<(), promptforge_gateway::ConfigError> {
    /// let config = Config::load(Path::new("gateway.toml"))?;
    /// let _ = config;
    /// # Ok(())
    /// # }
    /// ```
    pub fn load(path: &Path) -> Result<Config, crate::api_error::ConfigError> {
        crate::profile::load_path(path).map_err(crate::api_error::ConfigError::from)
    }

    /// Load a named profile from `dir` with recursive `include` resolution.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) when the profile is missing,
    /// includes cycle or exceed depth, or the resolved document fails config
    /// validation.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_gateway::{Config, ProfileName};
    /// use std::path::Path;
    ///
    /// # fn demo() -> Result<(), Box<dyn std::error::Error>> {
    /// let name = ProfileName::parse("dev")?;
    /// let config = Config::load_profile(Path::new("/etc/promptforge/profiles"), &name)?;
    /// let _ = config;
    /// # Ok(())
    /// # }
    /// ```
    pub fn load_profile(
        dir: &Path,
        name: &crate::profile::ProfileName,
    ) -> Result<Config, crate::api_error::ConfigError> {
        crate::profile::load_named(dir, name.as_str()).map_err(crate::api_error::ConfigError::from)
    }

    /// The server bind address.
    #[must_use]
    pub(crate) fn bind_addr(&self) -> SocketAddr {
        self.server.bind
    }

    /// A clone of the server's bearer key.
    #[must_use]
    pub(crate) fn server_key(&self) -> Secret {
        self.server.key.clone()
    }

    /// The web-search configuration, when `[tools.web_search]` is present.
    #[must_use]
    pub(crate) fn web_search_config(&self) -> Option<&WebSearchConfig> {
        self.tools
            .as_ref()
            .and_then(|tools| tools.web_search.as_ref())
    }

    /// Resolve the concurrency limit for an endpoint: explicit
    /// `concurrency`, else the referenced remote device's concurrency, else
    /// unlimited (`None`).
    #[must_use]
    pub(crate) fn endpoint_concurrency(&self, endpoint: &EndpointConfig) -> Option<usize> {
        if let Some(n) = endpoint.concurrency {
            return Some(n);
        }
        let device_id = endpoint.device.as_deref()?;
        self.devices
            .iter()
            .find(|d| d.id == device_id)
            .and_then(|d| d.concurrency)
    }

    /// Resolve lane concurrency for a local model. Defaults to 1 when no
    /// device/lane is declared.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] when the device or lane is missing.
    pub(crate) fn local_model_concurrency(
        &self,
        model: &LocalModelConfig,
    ) -> Result<usize, ConfigError> {
        match (&model.device, &model.lane) {
            (None, None) => Ok(1),
            (Some(device_id), Some(lane_id)) => {
                let device = self
                    .devices
                    .iter()
                    .find(|d| d.id == *device_id)
                    .ok_or_else(|| {
                        ConfigError::Validation(format!(
                            "local_model {} names undefined device {device_id}",
                            model.name
                        ))
                    })?;
                let lane = device
                    .lanes
                    .iter()
                    .find(|l| l.id == *lane_id)
                    .ok_or_else(|| {
                        ConfigError::Validation(format!(
                            "local_model {} names undefined lane {lane_id} on device {device_id}",
                            model.name
                        ))
                    })?;
                if lane.concurrency < 1 {
                    return Err(ConfigError::Validation(format!(
                        "device {device_id} lane {lane_id} concurrency must be at least 1"
                    )));
                }
                Ok(lane.concurrency)
            }
            _ => Err(ConfigError::Validation(format!(
                "local_model {} must set both device and lane, or neither",
                model.name
            ))),
        }
    }

    /// Interpolate, parse, and validate a configuration from a TOML string.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) for a malformed or unresolved
    /// interpolation, invalid TOML, or a failed semantic check.
    ///
    /// # Examples
    /// ```
    /// use promptforge_gateway::Config;
    ///
    /// let toml = r#"
    /// [server]
    /// bind = "127.0.0.1:8080"
    /// key = "secret"
    ///
    /// [[endpoint]]
    /// id = "e"
    /// protocol = "openai"
    /// base_url = "http://127.0.0.1:9"
    /// api_key = ""
    ///
    /// [[model]]
    /// name = "m"
    /// description = "a model"
    /// context = 8192
    /// upstream = "u"
    /// endpoints = ["e"]
    /// "#;
    /// let config = Config::from_toml_str(toml).unwrap();
    /// let _ = config;
    /// ```
    pub fn from_toml_str(raw: &str) -> Result<Config, crate::api_error::ConfigError> {
        Self::parse_toml(raw).map_err(crate::api_error::ConfigError::from)
    }

    /// Interpolate, parse, and validate, returning the internal error type.
    pub(crate) fn parse_toml(raw: &str) -> Result<Config, ConfigError> {
        let interpolated = interpolate(raw)?;
        let raw: RawConfig =
            toml::from_str(&interpolated).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let config = Config::from(raw);
        config.validate()?;
        Ok(config)
    }

    /// Check names are unique and every model references a defined endpoint.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] on a duplicate endpoint or model
    /// name, a model with an empty endpoint list, a model naming an undefined
    /// endpoint, an invalid `[[local_model]]`, `queue.max_depth` below 1, or
    /// an endpoint `concurrency` below 1.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.server.key.is_empty() {
            return Err(ConfigError::Validation(
                "server.key must not be empty".to_string(),
            ));
        }
        if self.queue.max_depth < 1 {
            return Err(ConfigError::Validation(
                "queue.max_depth must be at least 1".to_string(),
            ));
        }
        self.validate_devices()?;
        let endpoint_ids = self.validate_endpoints()?;
        self.validate_models(&endpoint_ids)?;
        Ok(())
    }

    fn validate_devices(&self) -> Result<(), ConfigError> {
        let mut device_ids = HashSet::new();
        for device in &self.devices {
            if device.id.is_empty() {
                return Err(ConfigError::Validation(
                    "device id must not be empty".to_string(),
                ));
            }
            if !device_ids.insert(device.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate device id {}",
                    device.id
                )));
            }
            if let Some(concurrency) = device.concurrency
                && concurrency < 1
            {
                return Err(ConfigError::Validation(format!(
                    "device {} concurrency must be at least 1",
                    device.id
                )));
            }
            let mut lane_ids = HashSet::new();
            for lane in &device.lanes {
                if lane.id.is_empty() {
                    return Err(ConfigError::Validation(format!(
                        "device {} lane id must not be empty",
                        device.id
                    )));
                }
                if !lane_ids.insert(lane.id.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "duplicate lane id {} on device {}",
                        lane.id, device.id
                    )));
                }
                if lane.concurrency < 1 {
                    return Err(ConfigError::Validation(format!(
                        "device {} lane {} concurrency must be at least 1",
                        device.id, lane.id
                    )));
                }
                if let Some(ref_id) = &lane.device
                    && ref_id != &device.id
                {
                    return Err(ConfigError::Validation(format!(
                        "lane {} device {ref_id} does not match parent device {}",
                        lane.id, device.id
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_endpoints(&self) -> Result<HashSet<&str>, ConfigError> {
        let mut endpoint_ids = HashSet::new();
        for endpoint in &self.endpoints {
            if !endpoint_ids.insert(endpoint.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate endpoint id {}",
                    endpoint.id
                )));
            }
            if let Some(concurrency) = endpoint.concurrency
                && concurrency < 1
            {
                return Err(ConfigError::Validation(format!(
                    "endpoint {} concurrency must be at least 1",
                    endpoint.id
                )));
            }
            if let Some(device_id) = &endpoint.device {
                let device = self.devices.iter().find(|d| d.id == *device_id);
                let Some(device) = device else {
                    return Err(ConfigError::Validation(format!(
                        "endpoint {} names undefined device {device_id}",
                        endpoint.id
                    )));
                };
                if device.kind != DeviceKind::Remote {
                    return Err(ConfigError::Validation(format!(
                        "endpoint {} references non-remote device {device_id}",
                        endpoint.id
                    )));
                }
            }
        }
        Ok(endpoint_ids)
    }

    fn validate_models(&self, endpoint_ids: &HashSet<&str>) -> Result<(), ConfigError> {
        let mut model_names = HashSet::new();
        for model in &self.models {
            if !model_names.insert(model.name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate model name {}",
                    model.name
                )));
            }
            if model.name.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "model name must not be empty".to_string(),
                ));
            }
            if model.description.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "model {} description must not be empty",
                    model.name
                )));
            }
            if model.upstream.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "model {} upstream must not be empty",
                    model.name
                )));
            }
            if model.context == 0 {
                return Err(ConfigError::Validation(format!(
                    "model {} context must be greater than zero",
                    model.name
                )));
            }
            if model.default_max_tokens == Some(0) {
                return Err(ConfigError::Validation(format!(
                    "model {} default_max_tokens must be greater than zero",
                    model.name
                )));
            }
            if model.endpoints.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "model {} has no endpoints",
                    model.name
                )));
            }
            let mut seen_endpoints = HashSet::new();
            for endpoint in &model.endpoints {
                if !endpoint_ids.contains(endpoint.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "model {} names undefined endpoint {endpoint}",
                        model.name
                    )));
                }
                if !seen_endpoints.insert(endpoint.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "model {} lists duplicate endpoint {endpoint}",
                        model.name
                    )));
                }
            }
        }

        self.validate_local_models(&mut model_names)
    }

    fn validate_local_models<'a>(
        &'a self,
        model_names: &mut HashSet<&'a str>,
    ) -> Result<(), ConfigError> {
        for local_model in &self.local_models {
            if local_model.name.is_empty() {
                return Err(ConfigError::Validation(
                    "local_model name must not be empty".to_string(),
                ));
            }
            if !model_names.insert(local_model.name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate model name {}",
                    local_model.name
                )));
            }
            if local_model.description.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "local_model {} description must not be empty",
                    local_model.name
                )));
            }
            if local_model.source.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "local_model {} source must not be empty",
                    local_model.name
                )));
            }
            if local_model.context < 1 {
                return Err(ConfigError::Validation(format!(
                    "local_model {} context must be at least 1",
                    local_model.name
                )));
            }
            if local_model.n_predict < 1 {
                return Err(ConfigError::Validation(format!(
                    "local_model {} n_predict must be at least 1",
                    local_model.name
                )));
            }
            if local_model.cache_type_k.is_empty() || local_model.cache_type_v.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "local_model {} cache_type_k/v must not be empty",
                    local_model.name
                )));
            }
            if let Some(sha) = &local_model.sha256
                && !is_sha256_hex(sha)
            {
                return Err(ConfigError::Validation(format!(
                    "local_model {} sha256 must be 64 lowercase hex characters",
                    local_model.name
                )));
            }
            self.local_model_concurrency(local_model)?;
        }
        Ok(())
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// Expand `${VAR}` from the environment; `$$` is a literal `$`.
fn interpolate(input: &str) -> Result<String, ConfigError> {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                out.push('$');
            }
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for nc in chars.by_ref() {
                    if nc == '}' {
                        closed = true;
                        break;
                    }
                    name.push(nc);
                }
                if !closed {
                    return Err(ConfigError::Interpolation(
                        "unclosed ${...} interpolation".to_string(),
                    ));
                }
                let value =
                    std::env::var(&name).map_err(|_| ConfigError::UnresolvedVar(name.clone()))?;
                out.push_str(&value);
            }
            _ => out.push('$'),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = ""

[[model]]
name = "m1"
description = "a small test model"
context = 8192
upstream = "u1"
endpoints = ["anthropic"]
"#;

    #[test]
    fn parses_a_valid_config() {
        let config = Config::from_toml_str(SAMPLE).unwrap();
        assert_eq!(config.endpoints.len(), 1);
        assert_eq!(config.models[0].name, "m1");
        assert_eq!(config.models[0].description, "a small test model");
        assert_eq!(config.models[0].context, 8192);
        assert_eq!(config.models[0].thinking, ThinkingMode::Never);
        assert_eq!(config.models[0].upstream, "u1");
    }

    #[test]
    fn rejects_model_missing_description() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
context = 8192
upstream = "u"
endpoints = ["anthropic"]
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn rejects_model_missing_context() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
upstream = "u"
endpoints = ["anthropic"]
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn rejects_empty_server_key() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = ""

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn rejects_unknown_model_key() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["anthropic"]
mystery = true
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn parses_thinking_modes() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
thinking = "switchable"
upstream = "u"
endpoints = ["anthropic"]
"#;
        let config = Config::from_toml_str(toml).unwrap();
        assert_eq!(config.models[0].thinking, ThinkingMode::Switchable);
    }

    #[test]
    fn interpolates_and_escapes() {
        // SAFETY-free: reading is fine; this test sets no env vars.
        assert_eq!(interpolate("a$$b").unwrap(), "a$b");
        assert_eq!(interpolate("no vars here").unwrap(), "no vars here");
    }

    #[test]
    fn unresolved_variable_is_an_error() {
        let missing = "${PROMPTFORGE_DEFINITELY_UNSET_VAR_XYZ}";
        assert!(matches!(
            interpolate(missing),
            Err(ConfigError::UnresolvedVar(_))
        ));
    }

    #[test]
    fn unclosed_interpolation_is_an_error() {
        assert!(matches!(
            interpolate("${OPEN"),
            Err(ConfigError::Interpolation(_))
        ));
    }

    #[test]
    fn rejects_duplicate_endpoint_names() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "dup"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[endpoint]]
id = "dup"
protocol = "openai"
base_url = "http://b"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["dup"]
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn rejects_model_naming_undefined_endpoint() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "real"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["ghost"]
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn rejects_model_with_no_endpoints() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "real"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = []
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn parses_web_search_tool_config() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = ""

[[model]]
name = "m1"
description = "a small test model"
context = 8192
upstream = "u1"
endpoints = ["anthropic"]

[tools.web_search]
provider = "brave"
api_key = "secret-key"
"#;
        let config = Config::from_toml_str(toml).unwrap();
        let tools = config.tools.expect("tools section present");
        let web_search = tools.web_search.expect("web_search section present");
        assert_eq!(web_search.provider, SearchProvider::Brave);
        assert_eq!(web_search.api_key.expose(), "secret-key");
        assert_eq!(web_search.base_url, "https://api.search.brave.com/res/v1");
        assert_eq!(web_search.default_count, 10);
        assert_eq!(web_search.max_count, 20);
        assert_eq!(web_search.max_per_host, 2);
        assert_eq!(web_search.default_freshness, "");
        assert_eq!(web_search.default_safesearch, "");
        assert!(web_search.strip_tracking);
    }

    #[test]
    fn parses_web_search_tool_config_explicit_defaults() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = ""

[[model]]
name = "m1"
description = "a small test model"
context = 8192
upstream = "u1"
endpoints = ["anthropic"]

[tools.web_search]
provider = "brave"
api_key = "secret-key"
default_count = 5
max_count = 15
max_per_host = 3
default_freshness = "pw"
default_safesearch = "moderate"
strip_tracking = false
"#;
        let config = Config::from_toml_str(toml).unwrap();
        let tools = config.tools.expect("tools section present");
        let web_search = tools.web_search.expect("web_search section present");
        assert_eq!(web_search.default_count, 5);
        assert_eq!(web_search.max_count, 15);
        assert_eq!(web_search.max_per_host, 3);
        assert_eq!(web_search.default_freshness, "pw");
        assert_eq!(web_search.default_safesearch, "moderate");
        assert!(!web_search.strip_tracking);
    }

    #[test]
    fn parses_config_without_tools_section() {
        let config = Config::from_toml_str(SAMPLE).unwrap();
        assert!(config.tools.is_none());
    }

    #[test]
    fn secret_redacts() {
        let s = Secret::from("hunter2".to_string());
        assert_eq!(format!("{s}"), "redacted");
        assert_eq!(format!("{s:?}"), "Secret(redacted)");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn parses_queue_and_endpoint_concurrency() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[queue]
max_depth = 50
fair_scheduling = false

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = ""
concurrency = 4

[[model]]
name = "m1"
description = "a small test model"
context = 8192
upstream = "u1"
endpoints = ["anthropic"]
"#;
        let config = Config::from_toml_str(toml).unwrap();
        assert_eq!(config.queue.max_depth, 50);
        assert!(!config.queue.fair_scheduling);
        assert_eq!(config.endpoints[0].concurrency, Some(4));
    }

    #[test]
    fn queue_defaults_when_section_absent() {
        let config = Config::from_toml_str(SAMPLE).unwrap();
        assert_eq!(config.queue.max_depth, 100);
        assert!(config.queue.fair_scheduling);
        assert_eq!(config.endpoints[0].concurrency, None);
    }

    #[test]
    fn rejects_zero_endpoint_concurrency() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""
concurrency = 0

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["anthropic"]
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn rejects_zero_queue_max_depth() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[queue]
max_depth = 0

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["anthropic"]
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn parses_local_model_with_defaults() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[local_model]]
name = "qwen-local"
description = "A careful analysis model suited to structured reasoning and long-context review"
source = "https://example.com/model.gguf"
context = 65536
thinking = "never"
"#;
        let config = Config::from_toml_str(toml).unwrap();
        assert!(config.endpoints.is_empty());
        assert!(config.models.is_empty());
        assert_eq!(config.local_models.len(), 1);
        let model = &config.local_models[0];
        assert_eq!(model.name, "qwen-local");
        assert_eq!(model.context, 65536);
        assert_eq!(model.thinking, ThinkingMode::Never);
        assert_eq!(model.gpu_layers, 99);
        assert!(model.flash_attention);
        assert_eq!(model.cache_type_k, "q8_0");
        assert_eq!(model.cache_type_v, "q4_0");
        assert_eq!(model.n_predict, 8192);
        assert!(model.sha256.is_none());
        assert!(config.local.cache_dir.is_none());
    }

    #[test]
    fn parses_local_model_knobs_and_cache_dir() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[local]
cache_dir = "/tmp/pf-models"

[[local_model]]
name = "qwen-local"
description = "prose"
source = "https://example.com/model.gguf"
sha256 = "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8"
context = 4096
gpu_layers = 40
flash_attention = false
cache_type_k = "f16"
cache_type_v = "f16"
n_predict = 256
"#;
        let config = Config::from_toml_str(toml).unwrap();
        assert_eq!(config.local.cache_dir.as_deref(), Some("/tmp/pf-models"));
        let model = &config.local_models[0];
        assert_eq!(
            model.sha256.as_deref(),
            Some("03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8")
        );
        assert_eq!(model.gpu_layers, 40);
        assert!(!model.flash_attention);
        assert_eq!(model.cache_type_k, "f16");
        assert_eq!(model.n_predict, 256);
    }

    #[test]
    fn rejects_duplicate_name_across_remote_and_local() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "shared"
description = "remote"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "shared"
description = "local"
source = "https://example.com/model.gguf"
context = 4096
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn rejects_invalid_local_model_sha256() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[local_model]]
name = "q"
description = "prose"
source = "https://example.com/model.gguf"
sha256 = "not-a-digest"
context = 4096
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn rejects_empty_local_model_source() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[local_model]]
name = "q"
description = "prose"
source = ""
context = 4096
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn parses_devices_lanes_and_endpoint_device() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[device]]
id = "anthropic"
type = "remote"
concurrency = 7

[[device]]
id = "local-gpu"
type = "local"

[[device.lane]]
device = "local-gpu"
id = "generative"
concurrency = 1

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""
device = "anthropic"

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["anthropic"]

[[local_model]]
name = "q"
description = "prose"
source = "https://example.com/m.gguf"
device = "local-gpu"
lane = "generative"
context = 4096
"#;
        let config = Config::from_toml_str(toml).unwrap();
        assert_eq!(config.devices.len(), 2);
        assert_eq!(config.devices[0].kind, DeviceKind::Remote);
        assert_eq!(config.devices[0].concurrency, Some(7));
        assert_eq!(config.devices[1].lanes.len(), 1);
        assert_eq!(config.devices[1].lanes[0].id, "generative");
        assert_eq!(config.endpoint_concurrency(&config.endpoints[0]), Some(7));
        assert_eq!(
            config
                .local_model_concurrency(&config.local_models[0])
                .unwrap(),
            1
        );
    }

    #[test]
    fn rejects_endpoint_naming_undefined_device() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""
device = "missing"

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]
"#;
        assert!(matches!(
            Config::parse_toml(toml),
            Err(ConfigError::Validation(_))
        ));
    }
}
