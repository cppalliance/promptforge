//! Gateway configuration: `gateway.toml` parsing, `${VAR}` interpolation, and
//! semantic validation.

use std::collections::HashSet;
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;

use serde::Deserialize;

use crate::error::ConfigError;

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
pub enum Protocol {
    /// The OpenAI `/chat/completions` shape.
    Openai,
}

/// The whole gateway configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Server bind address and shared token.
    pub server: ServerConfig,
    /// The configured backends.
    #[serde(rename = "endpoint")]
    pub endpoints: Vec<EndpointConfig>,
    /// The routing table from model name to backend.
    #[serde(rename = "model")]
    pub models: Vec<ModelConfig>,
    /// Optional built-in tool configuration. Absent when no `[tools]` section
    /// is present.
    #[serde(default)]
    pub tools: Option<ToolsConfig>,
}

/// Server-level settings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// The socket address to bind.
    pub bind: SocketAddr,
    /// The shared bearer token every `/v1/*` request must present.
    pub token: Secret,
}

/// One backend the gateway can forward to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    /// The endpoint's id: an operator-chosen handle referenced by `[[model]]`
    /// entries. Distinct from a model's caller-facing `name`.
    pub id: String,
    /// The wire protocol this endpoint speaks.
    pub protocol: Protocol,
    /// The backend base URL (a trailing slash is trimmed).
    pub base_url: String,
    /// The credential sent to this backend.
    pub api_key: Secret,
}

/// One model name and the backend it resolves to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// The name callers request and that a slot resolves to.
    pub name: String,
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
pub struct ToolsConfig {
    /// The web-search tool configuration. Absent when no `[tools.web_search]`
    /// section is present.
    #[serde(default)]
    pub web_search: Option<WebSearchConfig>,
}

/// Configuration for the web-search tool.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchConfig {
    /// The search provider backing the tool.
    pub provider: SearchProvider,
    /// The credential sent to the search provider.
    pub api_key: Secret,
}

/// A web-search provider. v0 supports only Brave.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SearchProvider {
    /// The Brave Search API.
    Brave,
}

impl Config {
    /// Load, interpolate, parse, and validate a configuration file.
    ///
    /// # Errors
    /// Returns [`ConfigError`] if the file cannot be read, an interpolation is
    /// malformed or references an unset variable, the TOML is invalid, or a
    /// semantic check fails.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Config::from_toml_str(&raw)
    }

    /// Interpolate, parse, and validate a configuration from a TOML string.
    ///
    /// # Errors
    /// Returns [`ConfigError`] for a malformed or unresolved interpolation,
    /// invalid TOML, or a failed semantic check.
    pub fn from_toml_str(raw: &str) -> Result<Config, ConfigError> {
        let interpolated = interpolate(raw)?;
        let config: Config =
            toml::from_str(&interpolated).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// Check names are unique and every model references a defined endpoint.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] on a duplicate endpoint or model
    /// name, a model with an empty endpoint list, or a model naming an
    /// undefined endpoint.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut endpoint_ids = HashSet::new();
        for endpoint in &self.endpoints {
            if !endpoint_ids.insert(endpoint.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate endpoint id {}",
                    endpoint.id
                )));
            }
        }

        let mut model_names = HashSet::new();
        for model in &self.models {
            if !model_names.insert(model.name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate model name {}",
                    model.name
                )));
            }
            if model.endpoints.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "model {} has no endpoints",
                    model.name
                )));
            }
            for endpoint in &model.endpoints {
                if !endpoint_ids.contains(endpoint.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "model {} names undefined endpoint {endpoint}",
                        model.name
                    )));
                }
            }
        }
        Ok(())
    }
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
token = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = ""

[[model]]
name = "m1"
upstream = "u1"
endpoints = ["anthropic"]
"#;

    #[test]
    fn parses_a_valid_config() {
        let config = Config::from_toml_str(SAMPLE).unwrap();
        assert_eq!(config.endpoints.len(), 1);
        assert_eq!(config.models[0].name, "m1");
        assert_eq!(config.models[0].upstream, "u1");
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
token = "t"

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
upstream = "u"
endpoints = ["dup"]
"#;
        assert!(matches!(
            Config::from_toml_str(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn rejects_model_naming_undefined_endpoint() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[endpoint]]
id = "real"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
upstream = "u"
endpoints = ["ghost"]
"#;
        assert!(matches!(
            Config::from_toml_str(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn rejects_model_with_no_endpoints() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[endpoint]]
id = "real"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
upstream = "u"
endpoints = []
"#;
        assert!(matches!(
            Config::from_toml_str(toml),
            Err(ConfigError::Validation(_))
        ));
    }

    #[test]
    fn parses_web_search_tool_config() {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = ""

[[model]]
name = "m1"
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
}
