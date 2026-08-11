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

mod interpolate;
#[cfg(test)]
mod tests;
mod types;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::ConfigError;

use self::interpolate::interpolate_document;
pub(crate) use self::types::{GatewayUrl, GlobPattern, PromptName, RelativePromptPath, Secret};

/// The whole MCP server configuration.
///
/// Opaque and validated: a `Config` exists only if it passed [`Config::load`]
/// or [`Config::from_toml_str`], so nothing downstream re-checks its
/// invariants. It carries no `Deserialize`; deserialization lands in the
/// private `RawConfig` and reaches this type only through a validating
/// `TryFrom`.
///
/// # Examples
/// ```
/// use promptforge_mcp_server::Config;
///
/// let config: Config = r#"
/// [server]
/// token = "shared-bearer"
///
/// [gateway]
/// url = "http://127.0.0.1:8081/v1"
/// key = "gateway-bearer"
/// "#
/// .parse()?;
/// assert_eq!(config, config.clone());
/// # Ok::<(), promptforge_mcp_server::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// Socket, shared token, concurrency, and the reload and timing settings.
    pub(crate) server: ServerConfig,
    /// Where the prompts live.
    pub(crate) paths: PathsConfig,
    /// The gateway every run's model calls go through.
    pub(crate) gateway: GatewayConfig,
    /// The globs that assemble the catalog.
    pub(crate) catalog: CatalogConfig,
    /// Per-prompt exceptions to the globs, keyed by the prompt's frontmatter
    /// `name`.
    pub(crate) prompts: BTreeMap<PromptName, PromptConfig>,
}

/// The unvalidated shape `prompts.toml` deserializes into.
///
/// Kept private and separate from [`Config`] so no caller can build a
/// configuration that skipped validation: the only way across the boundary is
/// the `TryFrom` below, which constructs every validated newtype. This is what
/// makes an unvalidated `Config` unrepresentable rather than merely
/// discouraged.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    server: RawServerConfig,
    #[serde(default)]
    paths: PathsConfig,
    gateway: RawGatewayConfig,
    #[serde(default)]
    catalog: CatalogConfig,
    #[serde(default)]
    prompts: BTreeMap<PromptName, PromptConfig>,
}

impl TryFrom<RawConfig> for Config {
    type Error = ConfigError;

    fn try_from(raw: RawConfig) -> Result<Config, ConfigError> {
        // The shared bearer is optional, but present it must carry something:
        // an empty token would compare equal to a request presenting no
        // credential. The blank rejection lives in [`Secret`]; it is mapped
        // back onto the token here so the public error keeps its
        // [`EmptyToken`](crate::ConfigErrorKind::EmptyToken) kind and message.
        let token = raw
            .server
            .token
            .map(|value| Secret::try_from(value).map_err(|_| ConfigError::empty_token()))
            .transpose()?;
        // The gateway credential is required and read on every run, so a blank
        // one is refused where it is read rather than surfacing later as an
        // opaque authentication failure against the gateway. The endpoint's own
        // shape is enforced by [`GatewayUrl`]'s validating conversion.
        let key = Secret::try_from(raw.gateway.key)
            .map_err(|_| ConfigError::parse("[gateway].key must not be empty"))?;
        Ok(Config {
            server: ServerConfig {
                bind: raw.server.bind,
                allowed_hosts: raw.server.allowed_hosts,
                token,
                max_concurrent_runs: raw.server.max_concurrent_runs,
                admission_timeout: raw.server.admission_timeout,
                reply_deadline: raw.server.reply_deadline,
                retain_completed: raw.server.retain_completed,
                watch: raw.server.watch,
                watch_debounce: raw.server.watch_debounce,
            },
            paths: raw.paths,
            gateway: GatewayConfig {
                url: raw.gateway.url,
                key,
            },
            catalog: raw.catalog,
            prompts: raw.prompts,
        })
    }
}

/// The unvalidated shape of the `[server]` section.
///
/// Carries the shared token as a raw `String` so a blank one reaches the
/// validating [`TryFrom`](Config) rather than being rejected mid-deserialize,
/// which is what lets the public error keep its
/// [`EmptyToken`](crate::ConfigErrorKind::EmptyToken) kind.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServerConfig {
    #[serde(default = "default_bind")]
    bind: SocketAddr,
    #[serde(default)]
    allowed_hosts: Vec<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default = "default_max_concurrent_runs")]
    max_concurrent_runs: NonZeroUsize,
    #[serde(default = "default_admission_timeout", with = "humantime_serde")]
    admission_timeout: Duration,
    #[serde(default = "default_reply_deadline", with = "humantime_serde")]
    reply_deadline: Duration,
    #[serde(default = "default_retain_completed", with = "humantime_serde")]
    retain_completed: Duration,
    #[serde(default = "default_watch")]
    watch: bool,
    #[serde(default = "default_watch_debounce", with = "humantime_serde")]
    watch_debounce: Duration,
}

/// The unvalidated shape of the `[gateway]` section.
///
/// Carries the key as a raw `String` so a blank one reaches the validating
/// [`TryFrom`](Config) and the public error keeps its
/// [`Parse`](crate::ConfigErrorKind::Parse) kind and `[gateway].key` message.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGatewayConfig {
    url: GatewayUrl,
    key: String,
}

/// Server-level settings.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct ServerConfig {
    /// The socket address the streamable-HTTP transport binds.
    pub(crate) bind: SocketAddr,
    /// The host authorities inbound `Host` headers are validated against on the
    /// streamable-HTTP transport, so a bound socket cannot be reached under a
    /// name the operator did not intend (the DNS-rebinding defence).
    ///
    /// Empty is the secure default only for a loopback bind, where it means the
    /// loopback names `localhost`, `127.0.0.1`, and `::1`. A non-loopback bind
    /// with an empty list is refused, because the reachable-host policy would
    /// otherwise silently contradict the bind: enumerate the public authorities
    /// (`["example.com", "example.com:8080"]`) instead. Read by the HTTP
    /// transport alone; stdio ignores it.
    pub(crate) allowed_hosts: Vec<String>,
    /// The shared bearer token every `/mcp` request must present.
    ///
    /// Optional, because the token is a property of the HTTP surface alone:
    /// `serve` refuses to bind without one and `serve --stdio` never reads it.
    /// A `${VAR}` here that the environment does not set leaves the token
    /// absent rather than failing the load, so a local stdio install is not
    /// stopped by a credential its transport never reads. Present, it must
    /// carry something.
    pub(crate) token: Option<Secret>,
    /// How many prompts may run at once before a call waits for admission.
    pub(crate) max_concurrent_runs: NonZeroUsize,
    /// How long a call waits for a run slot before it is refused.
    pub(crate) admission_timeout: Duration,
    /// How long a call blocks before it returns a `running` result and leaves
    /// the run going in the background. Keep it under the client's own call
    /// ceiling: Cursor's remote calls fail at about 300 seconds.
    pub(crate) reply_deadline: Duration,
    /// How long a finished run stays collectable before it is evicted.
    pub(crate) retain_completed: Duration,
    /// Whether to re-read prompts on save, so writing a prompt is an
    /// edit-and-call loop rather than an edit-restart-call one.
    pub(crate) watch: bool,
    /// How long the watcher waits for filesystem events to settle.
    pub(crate) watch_debounce: Duration,
}

/// Filesystem locations.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct PathsConfig {
    /// The prompts directory. Catalog patterns and a named block's `file` are
    /// both relative to it.
    #[serde(default = "default_prompts_dir")]
    pub(crate) prompts: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        PathsConfig {
            prompts: default_prompts_dir(),
        }
    }
}

/// The gateway a run's model calls go through.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct GatewayConfig {
    /// The gateway base URL, for example `http://127.0.0.1:8081/v1`.
    pub(crate) url: GatewayUrl,
    /// The shared key the gateway requires.
    pub(crate) key: Secret,
}

/// The globs that assemble the catalog.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct CatalogConfig {
    /// Patterns to include, relative to the prompts directory. `*` does not
    /// cross a separator and `**` does. Empty enumerates by hand.
    #[serde(default)]
    pub(crate) include: Vec<GlobPattern>,
    /// Patterns to subtract from the included set.
    #[serde(default)]
    pub(crate) exclude: Vec<GlobPattern>,
}

/// One `[prompts.NAME]` block: an exception to the globs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct PromptConfig {
    /// Whether the prompt is published at all. `false` drops one the globs
    /// caught.
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    /// A path relative to the prompts directory, for a file no glob matches.
    /// Absent means the block is an exception to a globbed prompt.
    #[serde(default)]
    pub(crate) file: Option<RelativePromptPath>,
}

/// The largest a `prompts.toml` may be. A configuration is a handful of
/// sections; anything past a few mebibytes is a wrong file or a mistake, and
/// reading it unbounded would let a stray path pull an arbitrarily large file
/// into memory at boot. Four mebibytes leaves generous headroom over any real
/// configuration.
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

impl Config {
    /// Loads, interpolates, and parses a configuration file.
    ///
    /// The read is capped at four mebibytes: a file larger than that is refused
    /// rather than read into memory, since a real configuration is a handful of
    /// sections and anything larger is a wrong path or a mistake.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] whose [`kind`](ConfigError::kind) is
    /// [`Read`](crate::ConfigErrorKind::Read) if `path` is unreadable,
    /// [`Interpolation`](crate::ConfigErrorKind::Interpolation) or
    /// [`UnresolvedVar`](crate::ConfigErrorKind::UnresolvedVar) if a `${VAR}` is
    /// malformed or unset, [`Parse`](crate::ConfigErrorKind::Parse) if the TOML
    /// is invalid, carries an unknown key, or exceeds the size cap, and
    /// [`EmptyToken`](crate::ConfigErrorKind::EmptyToken) if `[server].token` is
    /// present and carries nothing.
    ///
    /// # Examples
    /// ```no_run
    /// use std::path::Path;
    /// use promptforge_mcp_server::Config;
    ///
    /// let config = Config::load(Path::new("prompts.toml"))?;
    /// # let _ = config;
    /// # Ok::<(), promptforge_mcp_server::ConfigError>(())
    /// ```
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        use std::io::Read as _;

        let file = std::fs::File::open(path)
            .map_err(|source| ConfigError::read(path.to_path_buf(), source))?;
        // Read one byte past the cap so an exactly-limit file still loads while
        // a larger one is detected without pulling the whole thing in.
        let mut raw = String::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_string(&mut raw)
            .map_err(|source| ConfigError::read(path.to_path_buf(), source))?;
        if raw.len() as u64 > MAX_CONFIG_BYTES {
            return Err(ConfigError::parse(format!(
                "config file {} exceeds the {MAX_CONFIG_BYTES}-byte limit",
                path.display()
            )));
        }
        Config::from_toml_str(&raw)
    }

    /// Interpolates and parses a configuration from a TOML string.
    ///
    /// The TOML is parsed first and every string value interpolated after, so
    /// an unset `${VAR}` is attributed to the field that carried it. That is
    /// what lets `[server].token` alone survive one: it is optional, and the
    /// transport that reads it refuses to bind without it.
    ///
    /// # Errors
    /// Returns a [`ConfigError`] whose [`kind`](ConfigError::kind) is
    /// [`Interpolation`](crate::ConfigErrorKind::Interpolation) or
    /// [`UnresolvedVar`](crate::ConfigErrorKind::UnresolvedVar) for a malformed
    /// or unset `${VAR}` outside `[server].token`,
    /// [`Parse`](crate::ConfigErrorKind::Parse) for invalid TOML or an unknown
    /// key, and [`EmptyToken`](crate::ConfigErrorKind::EmptyToken) for a
    /// `[server].token` that is present and empty or whitespace alone.
    ///
    /// # Examples
    /// ```
    /// let config = promptforge_mcp_server::Config::from_toml_str(
    ///     r#"
    /// [server]
    /// token = "shared-bearer"
    ///
    /// [gateway]
    /// url = "http://127.0.0.1:8081/v1"
    /// key = "gateway-bearer"
    /// "#,
    /// )?;
    /// let _config = config;
    /// # Ok::<(), promptforge_mcp_server::ConfigError>(())
    /// ```
    pub fn from_toml_str(raw: &str) -> Result<Config, ConfigError> {
        let mut document: toml::Table = toml::from_str(raw).map_err(ConfigError::parse_toml)?;
        interpolate_document(&mut document)?;
        let raw_config: RawConfig = toml::Value::Table(document)
            .try_into()
            .map_err(ConfigError::parse_toml)?;
        Config::try_from(raw_config)
    }
}

impl std::str::FromStr for Config {
    type Err = ConfigError;

    /// Parses a configuration from a TOML string, the same path as
    /// [`Config::from_toml_str`], so `s.parse::<Config>()` and
    /// `Config::from_toml_str(s)` agree.
    fn from_str(s: &str) -> Result<Config, ConfigError> {
        Config::from_toml_str(s)
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
