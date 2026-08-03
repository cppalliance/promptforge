//! `prompts.toml`: `${VAR}` interpolation, parsing, and the settings the rest of
//! the server reads.
//!
//! One file carries everything the service needs: the socket and the shared
//! token, the timings that keep a long run inside a client's call ceiling, the
//! prompts directory, the gateway the runs go through, and which prompts the
//! harness sees. The run is configured here rather than in the process
//! environment because setting an environment variable is `unsafe` under
//! edition 2024 and this workspace forbids unsafe, so a gateway client is
//! constructed explicitly from these values.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::ConfigError;

/// A secret string (the shared bearer or the gateway token) that never
/// serializes and redacts in both `Debug` and `Display`.
#[derive(Clone, Deserialize)]
#[serde(from = "String")]
pub struct Secret(String);

impl Secret {
    /// The secret's bytes. The one place a secret is read, when building auth.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is the empty string.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the secret carries nothing usable: empty, or only whitespace.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
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

/// How much of the harness's attention a prompt occupies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Expose {
    /// Its own entry in `tools/list`, named and described for the calling model
    /// to select directly. A promotion: a new or renamed direct tool is
    /// invisible until the client restarts.
    Tool,
    /// Reachable only through the built-in listing and retrieval tools, which
    /// costs one extra round trip and no permanent context slot.
    #[default]
    List,
}

/// The whole MCP server configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Config {
    /// Socket, shared token, concurrency, and the reload and timing settings.
    pub server: ServerConfig,
    /// Where the prompts live.
    #[serde(default)]
    pub paths: PathsConfig,
    /// The gateway every run's model calls go through.
    pub gateway: GatewayConfig,
    /// The globs that assemble the catalog.
    #[serde(default)]
    pub catalog: CatalogConfig,
    /// Per-prompt exceptions to the globs, keyed by the prompt's frontmatter
    /// `name`.
    #[serde(default)]
    pub prompts: BTreeMap<String, PromptConfig>,
}

/// Server-level settings.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ServerConfig {
    /// The socket address the streamable-HTTP transport binds.
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    /// The shared bearer token every `/mcp` request must present.
    pub token: Secret,
    /// How many prompts may run at once before a call waits for admission.
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: NonZeroUsize,
    /// How long a call waits for a run slot before it is refused.
    #[serde(default = "default_admission_timeout", with = "humantime_serde")]
    pub admission_timeout: Duration,
    /// How long a call blocks before it returns a `running` result and leaves
    /// the run going in the background. Keep it under the client's own call
    /// ceiling: Cursor's remote calls fail at about 300 seconds.
    #[serde(default = "default_reply_deadline", with = "humantime_serde")]
    pub reply_deadline: Duration,
    /// How long a finished run stays collectable before it is evicted.
    #[serde(default = "default_retain_completed", with = "humantime_serde")]
    pub retain_completed: Duration,
    /// Whether to re-read prompts on save, so writing a prompt is an
    /// edit-and-call loop rather than an edit-restart-call one.
    #[serde(default = "default_watch")]
    pub watch: bool,
    /// How long the watcher waits for filesystem events to settle.
    #[serde(default = "default_watch_debounce", with = "humantime_serde")]
    pub watch_debounce: Duration,
}

/// Filesystem locations.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PathsConfig {
    /// The prompts directory. Catalog patterns and a named block's `file` are
    /// both relative to it.
    #[serde(default = "default_prompts_dir")]
    pub prompts: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        PathsConfig {
            prompts: default_prompts_dir(),
        }
    }
}

/// The gateway a run's model calls go through.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GatewayConfig {
    /// The gateway base URL, for example `http://127.0.0.1:8081/v1`.
    pub url: String,
    /// The shared token the gateway requires.
    pub token: Secret,
    /// The model to request. Absent leaves the core's own default in place.
    #[serde(default)]
    pub model: Option<String>,
}

/// The globs that assemble the catalog, and the exposure a globbed prompt gets
/// when no named block says otherwise.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CatalogConfig {
    /// Patterns to include, relative to the prompts directory. `*` does not
    /// cross a separator and `**` does. Empty enumerates by hand.
    #[serde(default)]
    pub include: Vec<String>,
    /// Patterns to subtract from the included set.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// The exposure a globbed prompt gets absent a named block.
    #[serde(default)]
    pub default_expose: Expose,
}

/// One `[prompts.NAME]` block: an exception to the globs.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PromptConfig {
    /// The exposure for this prompt, overriding `default_expose`.
    #[serde(default)]
    pub expose: Option<Expose>,
    /// Whether the prompt is published at all. `false` drops one the globs
    /// caught.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// A path relative to the prompts directory, for a file no glob matches.
    /// Absent means the block is an exception to a globbed prompt.
    #[serde(default)]
    pub file: Option<PathBuf>,
}

impl Config {
    /// Loads, interpolates, and parses a configuration file.
    ///
    /// # Errors
    /// Returns [`ConfigError::Read`] if `path` is unreadable,
    /// [`ConfigError::Interpolation`] or [`ConfigError::UnresolvedVar`] if a
    /// `${VAR}` is malformed or unset, [`ConfigError::Parse`] if the TOML is
    /// invalid or carries an unknown key, and [`ConfigError::EmptyToken`] if
    /// `[server].token` carries nothing.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Config::from_toml_str(&raw)
    }

    /// Interpolates and parses a configuration from a TOML string.
    ///
    /// # Errors
    /// Returns [`ConfigError::Interpolation`] or [`ConfigError::UnresolvedVar`]
    /// for a malformed or unset `${VAR}`, [`ConfigError::Parse`] for invalid
    /// TOML or an unknown key, and [`ConfigError::EmptyToken`] for a
    /// `[server].token` that is empty or whitespace alone.
    ///
    /// # Examples
    /// ```
    /// let config = promptforge_mcp::Config::from_toml_str(
    ///     r#"
    /// [server]
    /// token = "shared-bearer"
    ///
    /// [gateway]
    /// url = "http://127.0.0.1:8081/v1"
    /// token = "gateway-bearer"
    /// "#,
    /// )?;
    /// assert_eq!(config.server.bind.port(), 9310);
    /// # Ok::<(), promptforge_mcp::ConfigError>(())
    /// ```
    pub fn from_toml_str(raw: &str) -> Result<Config, ConfigError> {
        let interpolated = interpolate(raw)?;
        let config: Config =
            toml::from_str(&interpolated).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// The checks a parsed configuration must pass before anything reads it.
    ///
    /// The shared bearer is the whole of it: an empty token would make a request
    /// presenting no credential compare equal to it, so the load refuses one
    /// rather than leaving the surface open to a typo. The bearer layer refuses
    /// an absent header on its own too, so the two defences are independent.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.server.token.is_blank() {
            return Err(ConfigError::EmptyToken);
        }
        Ok(())
    }
}

/// The default bind address, a loopback port well clear of the gateway's.
fn default_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 9310))
}

/// The default number of prompts that may run at once. Built by addition from
/// `MIN` so no fallible constructor stands between the literal and the value.
fn default_max_concurrent_runs() -> NonZeroUsize {
    NonZeroUsize::MIN.saturating_add(3)
}

/// The default wait for a run slot.
fn default_admission_timeout() -> Duration {
    Duration::from_secs(30)
}

/// The default block before a call returns a `running` result. Chosen to land
/// inside Cursor's 300-second wall with margin.
fn default_reply_deadline() -> Duration {
    Duration::from_secs(240)
}

/// The default window a finished run stays collectable.
fn default_retain_completed() -> Duration {
    Duration::from_secs(60 * 60)
}

/// Watching is on: a developer editing a prompt should not restart a service.
fn default_watch() -> bool {
    true
}

/// The default debounce window for filesystem events.
fn default_watch_debounce() -> Duration {
    Duration::from_millis(500)
}

/// The default prompts directory, relative to the process's working directory.
fn default_prompts_dir() -> PathBuf {
    PathBuf::from("prompts")
}

/// A named block publishes its prompt unless it says otherwise.
fn default_enabled() -> bool {
    true
}

/// Expands `${VAR}` from the process environment; `$$` is a literal `$`.
fn interpolate(input: &str) -> Result<String, ConfigError> {
    interpolate_with(input, &|name| std::env::var(name).ok())
}

/// Expands `${VAR}` through `lookup`, which answers `None` for an unset name.
fn interpolate_with(
    input: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, ConfigError> {
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
                let value = lookup(&name).ok_or(ConfigError::UnresolvedVar(name))?;
                out.push_str(&value);
            }
            _ => out.push('$'),
        }
    }
    Ok(out)
}
