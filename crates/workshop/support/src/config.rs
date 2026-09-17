//! `workshop.toml` loading: TOML parsing, `${VAR}` environment
//! interpolation, and defaults.
//!
//! Interpolation follows the promptforge convention (gateway-config CFG-007):
//! the TOML is parsed first and only string *values* are interpolated, so
//! `${VAR}` inside comments or keys is never expanded and an interpolated
//! value containing a quote, backslash, or newline cannot corrupt the
//! document. `$$` is a literal `$`. An unset variable interpolates to the
//! empty string: an empty `gateway.base_url` names no explicit gateway
//! (endpoint resolution attaches through the gateway discovery file or fails
//! plainly), and an empty `gateway.api_key` sends no `Authorization`
//! header.

use std::path::{Path, PathBuf};

/// Address the workshop server binds to when no override is given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7910";

/// Path [`Config::load`] reads when no override is given.
pub const DEFAULT_CONFIG_PATH: &str = "workshop.toml";

/// Workshop server configuration loaded from `workshop.toml`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Connection settings for the PromptForge gateway.
    pub gateway: GatewayConfig,
    /// HTTP server settings.
    #[serde(default)]
    pub server: ServerConfig,
    /// Agent-program discovery settings.
    #[serde(default)]
    pub agents: AgentsConfig,
}

impl Config {
    /// Loads and parses the workshop configuration from `path`.
    ///
    /// # Errors
    /// Returns [`ConfigError::NotFound`] if `path` does not exist,
    /// [`ConfigError::Read`] if `path` exists but cannot be read,
    /// [`ConfigError::Parse`] if the contents do not match the workshop
    /// schema, [`ConfigError::UnresolvedVar`] if a `${VAR}` names a variable
    /// whose value is not valid Unicode, and [`ConfigError::Interpolation`]
    /// if a `${...}` is malformed.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ConfigError::NotFound {
                    path: path.to_path_buf(),
                }
            } else {
                ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        Self::parse(&raw, Some(path))
    }

    /// Parses a workshop configuration from a TOML string.
    ///
    /// # Errors
    /// Returns [`ConfigError::Parse`] if `raw` is not valid TOML or does not
    /// match the workshop schema, [`ConfigError::UnresolvedVar`] if a
    /// `${VAR}` names a variable whose value is not valid Unicode, and
    /// [`ConfigError::Interpolation`] if a `${...}` is malformed.
    ///
    /// # Examples
    /// ```
    /// let config = workshop_support::Config::from_toml_str(
    ///     "[gateway]\nbase_url = \"http://127.0.0.1:8081\"\napi_key = \"k\"\n",
    /// )?;
    /// assert_eq!(config.server.bind, "127.0.0.1:7910");
    /// # Ok::<(), workshop_support::ConfigError>(())
    /// ```
    pub fn from_toml_str(raw: &str) -> Result<Self, ConfigError> {
        Self::parse(raw, None)
    }

    fn parse(raw: &str, path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut document: toml::Value =
            toml::from_str(raw).map_err(|source| ConfigError::Parse {
                path: path.map(Path::to_path_buf),
                source: Box::new(source),
            })?;
        interpolate_value(&mut document)?;
        let mut config: Self = document.try_into().map_err(|source| ConfigError::Parse {
            path: path.map(Path::to_path_buf),
            source: Box::new(source),
        })?;
        // An empty `gateway.base_url` is kept as-is: it is the not-explicit
        // signal endpoint resolution reads, so no default is filled here.
        // The path-shaped defaults anchor beside the config file. A config
        // parsed from a string has no file, so the anchor degrades to the
        // working directory.
        let anchor = path
            .and_then(Path::parent)
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        config.anchor_path_defaults(anchor);
        Ok(config)
    }

    /// Replaces the empty path-shaped defaults with paths anchored at
    /// `anchor`: an empty `server.state_dir` becomes `anchor` itself, and
    /// an empty `agents.path` becomes `agents/` under it. Explicit
    /// (non-empty) values are kept verbatim. Parsing applies this with the
    /// config file's directory; a host that builds a [`Config`] in code
    /// applies it with its own anchor to get the same defaults.
    pub fn anchor_path_defaults(&mut self, anchor: &Path) {
        if self.server.state_dir.as_os_str().is_empty() {
            self.server.state_dir = anchor.to_path_buf();
        }
        if self.agents.path.as_os_str().is_empty() {
            self.agents.path = anchor.join("agents");
        }
    }
}

/// Gateway connection settings: where the gateway listens and how to
/// authenticate to it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct GatewayConfig {
    /// Base URL of the gateway, for example `http://127.0.0.1:8081`. Empty
    /// names no explicit gateway: endpoint resolution attaches through the
    /// gateway discovery file, or fails plainly when no live file exists.
    pub base_url: String,
    /// Bearer key for the gateway API; supports `${VAR}` interpolation.
    pub api_key: String,
}

/// HTTP server settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Address the workshop server binds to.
    pub bind: String,
    /// When true, the server binary opens the system browser at its address
    /// once it is serving. The desktop shell sets up its own window and
    /// ignores this flag; it exists for the browser-tab frame.
    pub open_browser: bool,
    /// Directory holding the server's persistent state: agent session
    /// event logs live under `state_dir/sessions/`, and the per-profile
    /// model memory and boot orphan sweep anchor here. Defaults to the
    /// config file's own directory (`Config::parse` anchors the empty
    /// default there).
    pub state_dir: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_ADDR.to_string(),
            open_browser: false,
            state_dir: PathBuf::new(),
        }
    }
}

/// Agent-program discovery settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    /// Directory whose `.md` files are the launchable agent prompts,
    /// discovered by file stem. A `chat.md` in the directory shadows the
    /// embedded built-in chat agent; an existing `chat.md` that cannot
    /// be read surfaces an error rather than silently serving the
    /// built-in. Defaults to `agents/` beside the config file
    /// (`Config::parse` anchors the empty default there). A missing
    /// directory still offers the embedded `chat` built-in - a state,
    /// not an error.
    pub path: PathBuf,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
        }
    }
}

/// A workshop configuration load or parse failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The configuration file does not exist.
    #[non_exhaustive]
    #[error("config file not found: {}", path.display())]
    NotFound {
        /// The path that was expected.
        path: PathBuf,
    },

    /// The configuration file could not be read.
    #[non_exhaustive]
    #[error("read config {}", path.display())]
    Read {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration was not valid TOML or did not match the schema.
    #[non_exhaustive]
    #[error("parse config{}", parse_location(path.as_deref()))]
    Parse {
        /// The file the parse failure came from, when known.
        path: Option<PathBuf>,
        /// The underlying TOML error, boxed to hide the dependency type.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A `${VAR}` named an environment variable whose value is not valid
    /// Unicode. An unset variable is not an error; it interpolates to the
    /// empty string.
    #[non_exhaustive]
    #[error("environment variable {0} is not valid Unicode")]
    UnresolvedVar(String),

    /// A `${...}` interpolation was malformed (for example, unclosed).
    #[non_exhaustive]
    #[error("interpolation: {0}")]
    Interpolation(String),
}

/// Renders the optional parse-failure path as a ` (path)` suffix or empty.
fn parse_location(path: Option<&Path>) -> String {
    path.map(|p| format!(" ({})", p.display()))
        .unwrap_or_default()
}

/// Expands `${VAR}` from the environment; `$$` is a literal `$`. An unset
/// variable expands to the empty string; a variable whose value is not
/// valid Unicode is an error.
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
                match std::env::var(&name) {
                    Ok(value) => out.push_str(&value),
                    Err(std::env::VarError::NotPresent) => {}
                    Err(std::env::VarError::NotUnicode(_)) => {
                        return Err(ConfigError::UnresolvedVar(name.clone()));
                    }
                }
            }
            _ => out.push('$'),
        }
    }
    Ok(out)
}

/// Recursively interpolates `${VAR}` in every string leaf of a TOML value,
/// leaving keys and non-string scalars untouched.
fn interpolate_value(value: &mut toml::Value) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(text) => {
            *text = interpolate(text)?;
        }
        toml::Value::Array(items) => {
            for item in items {
                interpolate_value(item)?;
            }
        }
        toml::Value::Table(table) => {
            for (_, entry) in table.iter_mut() {
                interpolate_value(entry)?;
            }
        }
        _ => {}
    }
    Ok(())
}
