//! `workbench.toml` loading: TOML parsing, `${VAR}` environment
//! interpolation, and defaults.
//!
//! Interpolation follows the promptforge convention (gateway-config CFG-007):
//! the TOML is parsed first and only string *values* are interpolated, so
//! `${VAR}` inside comments or keys is never expanded and an interpolated
//! value containing a quote, backslash, or newline cannot corrupt the
//! document. `$$` is a literal `$`.

use std::path::{Path, PathBuf};

/// Path [`Config::load`] reads when no override is given.
pub const DEFAULT_CONFIG_PATH: &str = "workbench.toml";

/// Workbench server configuration loaded from `workbench.toml`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Config {
    /// Connection settings for the PromptForge gateway.
    pub gateway: GatewayConfig,
    /// Session tape settings.
    #[serde(default)]
    pub tape: TapeConfig,
    /// HTTP server settings.
    #[serde(default)]
    pub server: ServerConfig,
    /// Voice transcription settings.
    #[serde(default)]
    pub voice: VoiceConfig,
}

impl Config {
    /// Loads and parses the workbench configuration from `path`.
    ///
    /// # Errors
    /// Returns [`ConfigError::Read`] if `path` cannot be read (including a
    /// missing file), [`ConfigError::Parse`] if the contents do not match the
    /// workbench schema, [`ConfigError::UnresolvedVar`] if a `${VAR}`
    /// references an unset environment variable, and
    /// [`ConfigError::Interpolation`] if a `${...}` is malformed.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&raw, Some(path))
    }

    /// Builds configuration entirely from environment variables, used when
    /// no `workbench.toml` is found.
    ///
    /// # Environment variables
    ///
    /// - `PROMPTFORGE_GATEWAY_BASE_URL` - default `http://127.0.0.1:8081`
    /// - `PROMPTFORGE_GATEWAY_API_KEY` - **required**
    /// - `PROMPTFORGE_TAPE_PATH` - default `tape.jsonl`
    /// - `PROMPTFORGE_SERVER_BIND` - default `127.0.0.1:7910`
    /// - `PROMPTFORGE_SERVER_OPEN_BROWSER` - default `false`; accepts
    ///   `true` or `1`
    /// - `PROMPTFORGE_VOICE_INTERIM_MODEL` - default empty (disabled)
    /// - `PROMPTFORGE_VOICE_FINAL_MODEL` - default empty
    /// - `PROMPTFORGE_VOICE_WINDOW_SECONDS` - default `5`
    /// - `PROMPTFORGE_VOICE_INTERVAL_MS` - default `800`
    ///
    /// # Errors
    /// Returns [`ConfigError::MissingEnvVar`] when a required variable is
    /// not set, or [`ConfigError::InvalidEnvVar`] when a variable cannot be
    /// parsed as the expected type.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_env_lookup(|name| std::env::var(name).ok())
    }

    /// Builds configuration from a variable lookup function.
    ///
    /// This is the implementation behind [`from_env`](Self::from_env),
    /// factored out so tests can supply a synthetic environment.
    fn from_env_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let api_key =
            lookup("PROMPTFORGE_GATEWAY_API_KEY").ok_or_else(|| ConfigError::MissingEnvVar {
                name: "PROMPTFORGE_GATEWAY_API_KEY".to_string(),
            })?;
        let base_url = lookup("PROMPTFORGE_GATEWAY_BASE_URL")
            .unwrap_or_else(|| "http://127.0.0.1:8081".to_string());
        let tape_path = lookup("PROMPTFORGE_TAPE_PATH").unwrap_or_else(|| "tape.jsonl".to_string());
        let bind =
            lookup("PROMPTFORGE_SERVER_BIND").unwrap_or_else(|| "127.0.0.1:7910".to_string());
        let open_browser = match lookup("PROMPTFORGE_SERVER_OPEN_BROWSER") {
            None => false,
            Some(v) => v == "true" || v == "1",
        };
        let interim_model = lookup("PROMPTFORGE_VOICE_INTERIM_MODEL").unwrap_or_default();
        let final_model = lookup("PROMPTFORGE_VOICE_FINAL_MODEL").unwrap_or_default();
        let window_seconds = match lookup("PROMPTFORGE_VOICE_WINDOW_SECONDS") {
            None => DEFAULT_VOICE_WINDOW_SECONDS,
            Some(v) => v.parse::<u64>().map_err(|_| ConfigError::InvalidEnvVar {
                name: "PROMPTFORGE_VOICE_WINDOW_SECONDS".to_string(),
                reason: format!("expected an integer, got {v:?}"),
            })?,
        };
        let interval_ms = match lookup("PROMPTFORGE_VOICE_INTERVAL_MS") {
            None => DEFAULT_VOICE_INTERVAL_MS,
            Some(v) => v.parse::<u64>().map_err(|_| ConfigError::InvalidEnvVar {
                name: "PROMPTFORGE_VOICE_INTERVAL_MS".to_string(),
                reason: format!("expected an integer, got {v:?}"),
            })?,
        };

        Ok(Self {
            gateway: GatewayConfig { base_url, api_key },
            tape: TapeConfig {
                path: PathBuf::from(tape_path),
            },
            server: ServerConfig { bind, open_browser },
            voice: VoiceConfig {
                interim_model: PathBuf::from(interim_model),
                final_model: PathBuf::from(final_model),
                window_seconds,
                interval_ms,
            },
        })
    }

    /// Parses a workbench configuration from a TOML string.
    ///
    /// # Errors
    /// Returns [`ConfigError::Parse`] if `raw` is not valid TOML or does not
    /// match the workbench schema, [`ConfigError::UnresolvedVar`] if a
    /// `${VAR}` references an unset environment variable, and
    /// [`ConfigError::Interpolation`] if a `${...}` is malformed.
    ///
    /// # Examples
    /// ```
    /// let config = promptforge_wb_server::Config::from_toml_str(
    ///     "[gateway]\nbase_url = \"http://127.0.0.1:8081\"\napi_key = \"k\"\n",
    /// )?;
    /// assert_eq!(config.server.bind, "127.0.0.1:7910");
    /// # Ok::<(), promptforge_wb_server::ConfigError>(())
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
        let config: Self = document.try_into().map_err(|source| ConfigError::Parse {
            path: path.map(Path::to_path_buf),
            source: Box::new(source),
        })?;
        Ok(config)
    }
}

/// Gateway connection settings: where the gateway listens and how to
/// authenticate to it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct GatewayConfig {
    /// Base URL of the gateway, for example `http://127.0.0.1:8081`.
    pub base_url: String,
    /// Bearer key for the gateway API; supports `${VAR}` interpolation.
    pub api_key: String,
}

/// Session tape settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct TapeConfig {
    /// Path of the JSONL tape file.
    pub path: PathBuf,
}

impl Default for TapeConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("tape.jsonl"),
        }
    }
}

/// HTTP server settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Address the workbench server binds to.
    pub bind: String,
    /// When true, the server binary opens the system browser at its address
    /// once it is serving. The desktop shell sets up its own window and
    /// ignores this flag; it exists for the browser-tab frame.
    pub open_browser: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: crate::DEFAULT_ADDR.to_string(),
            open_browser: false,
        }
    }
}

/// Default sliding-window length for interim transcription, in seconds.
pub const DEFAULT_VOICE_WINDOW_SECONDS: u64 = 5;

/// Default interval between interim transcriptions, in milliseconds.
pub const DEFAULT_VOICE_INTERVAL_MS: u64 = 800;

/// Voice transcription settings: whisper model paths and the interim loop's
/// window and cadence.
///
/// Transcription is enabled by setting `interim_model`; with no model paths
/// configured the `/voice` endpoint still captures and counts PCM but emits
/// empty transcripts. Setting `final_model` enables the pipelined final
/// pass: completed speech segments are transcribed with the final model in
/// the background while the user talks, and on stop only the unprocessed
/// tail remains. Without it, the final transcript falls back to one last
/// interim-model window.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Path to the GGML/GGUF whisper model for interim (streaming)
    /// transcription. Empty disables transcription.
    pub interim_model: PathBuf,
    /// Path to the whisper model for the pipelined final pass over a take.
    /// Empty disables the final pass; the final transcript then comes from
    /// the interim model.
    pub final_model: PathBuf,
    /// Seconds of trailing audio each interim pass transcribes.
    pub window_seconds: u64,
    /// Milliseconds between interim passes while a take is recording.
    pub interval_ms: u64,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            interim_model: PathBuf::new(),
            final_model: PathBuf::new(),
            window_seconds: DEFAULT_VOICE_WINDOW_SECONDS,
            interval_ms: DEFAULT_VOICE_INTERVAL_MS,
        }
    }
}

impl VoiceConfig {
    /// Returns true when an interim model path is configured.
    #[must_use]
    pub fn enabled(&self) -> bool {
        !self.interim_model.as_os_str().is_empty()
    }
}

/// A workbench configuration load or parse failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
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

    /// A `${VAR}` referenced an environment variable that was not set.
    #[non_exhaustive]
    #[error("unresolved environment variable {0}")]
    UnresolvedVar(String),

    /// A `${...}` interpolation was malformed (for example, unclosed).
    #[non_exhaustive]
    #[error("interpolation: {0}")]
    Interpolation(String),

    /// A required environment variable was not set (env-only config path).
    #[non_exhaustive]
    #[error("required environment variable {name} is not set")]
    MissingEnvVar {
        /// The variable that was expected.
        name: String,
    },

    /// An environment variable could not be parsed as the expected type.
    #[non_exhaustive]
    #[error("environment variable {name}: {reason}")]
    InvalidEnvVar {
        /// The variable that was malformed.
        name: String,
        /// What went wrong.
        reason: String,
    },
}

/// Renders the optional parse-failure path as a ` (path)` suffix or empty.
fn parse_location(path: Option<&Path>) -> String {
    path.map(|p| format!(" ({})", p.display()))
        .unwrap_or_default()
}

/// Expands `${VAR}` from the environment; `$$` is a literal `$`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture_and_interpolates_from_environment() {
        let path_value = std::env::var("PATH").expect("PATH is set on every supported platform");
        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "${PATH}"
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert_eq!(config.gateway.base_url, "http://127.0.0.1:8081");
        assert_eq!(config.gateway.api_key, path_value);
    }

    #[test]
    fn defaults_fill_tape_and_server() {
        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert_eq!(config.tape.path, PathBuf::from("tape.jsonl"));
        assert_eq!(config.server.bind, "127.0.0.1:7910");
    }

    #[test]
    fn explicit_sections_override_defaults() {
        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"

[tape]
path = "session.jsonl"

[server]
bind = "127.0.0.1:9000"
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert_eq!(config.tape.path, PathBuf::from("session.jsonl"));
        assert_eq!(config.server.bind, "127.0.0.1:9000");
    }

    #[test]
    fn open_browser_defaults_to_false_and_parses_when_set() {
        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert!(!config.server.open_browser, "default is off");

        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"

[server]
open_browser = true
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert!(config.server.open_browser);
        assert_eq!(config.server.bind, "127.0.0.1:7910", "bind still defaults");
    }

    #[test]
    fn voice_defaults_disable_transcription() {
        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert!(!config.voice.enabled(), "no model paths means disabled");
        assert_eq!(config.voice.window_seconds, DEFAULT_VOICE_WINDOW_SECONDS);
        assert_eq!(config.voice.interval_ms, DEFAULT_VOICE_INTERVAL_MS);
    }

    #[test]
    fn voice_section_parses_model_paths_and_tuning() {
        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"

[voice]
interim_model = "models/ggml-tiny.en.bin"
final_model = "models/ggml-small.en.bin"
window_seconds = 8
interval_ms = 500
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert!(config.voice.enabled());
        assert_eq!(
            config.voice.interim_model,
            PathBuf::from("models/ggml-tiny.en.bin")
        );
        assert_eq!(
            config.voice.final_model,
            PathBuf::from("models/ggml-small.en.bin")
        );
        assert_eq!(config.voice.window_seconds, 8);
        assert_eq!(config.voice.interval_ms, 500);
    }

    #[test]
    fn double_dollar_is_literal() {
        let raw = "[gateway]\nbase_url = \"http://x\"\napi_key = \"cost $$5\"\n";
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert_eq!(config.gateway.api_key, "cost $5");
    }

    #[test]
    fn unset_variable_is_an_error() {
        let raw =
            "[gateway]\nbase_url = \"http://x\"\napi_key = \"${PFG_WB_DEFINITELY_UNSET_XYZ}\"\n";
        let err = Config::from_toml_str(raw).expect_err("unset variable must fail");
        assert!(
            matches!(err, ConfigError::UnresolvedVar(ref name) if name == "PFG_WB_DEFINITELY_UNSET_XYZ"),
            "expected UnresolvedVar, got {err:?}"
        );
    }

    #[test]
    fn unclosed_interpolation_is_an_error() {
        let raw = "[gateway]\nbase_url = \"http://x\"\napi_key = \"${UNCLOSED\"\n";
        let err = Config::from_toml_str(raw).expect_err("unclosed interpolation must fail");
        assert!(
            matches!(err, ConfigError::Interpolation(_)),
            "expected Interpolation, got {err:?}"
        );
    }

    #[test]
    fn missing_gateway_section_is_an_error() {
        let err = Config::from_toml_str("[server]\nbind = \"127.0.0.1:9000\"\n")
            .expect_err("gateway section is required");
        assert!(
            matches!(err, ConfigError::Parse { .. }),
            "expected Parse, got {err:?}"
        );
    }

    #[test]
    fn missing_file_names_the_expected_path() {
        let err = Config::load(Path::new("definitely-missing-workbench.toml"))
            .expect_err("missing file must fail");
        assert!(
            matches!(err, ConfigError::Read { .. }),
            "expected Read, got {err:?}"
        );
        assert!(
            err.to_string()
                .contains("definitely-missing-workbench.toml"),
            "error names the path: {err}"
        );
    }

    #[test]
    fn parse_error_names_the_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("broken-workbench.toml");
        std::fs::write(&path, "[gateway\n").expect("write fixture");
        let err = Config::load(&path).expect_err("malformed TOML must fail");
        assert!(
            matches!(err, ConfigError::Parse { .. }),
            "expected Parse, got {err:?}"
        );
        assert!(
            err.to_string().contains("broken-workbench.toml"),
            "error names the path: {err}"
        );
    }

    #[test]
    fn load_reads_and_parses_a_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("workbench.toml");
        std::fs::write(
            &path,
            "[gateway]\nbase_url = \"http://127.0.0.1:8081\"\napi_key = \"k\"\n",
        )
        .expect("write fixture");
        let config = Config::load(&path).expect("fixture loads");
        assert_eq!(config.gateway.api_key, "k");
    }

    #[test]
    fn from_env_produces_valid_config_with_required_vars() {
        use std::collections::HashMap;
        let mut env: HashMap<&str, &str> = HashMap::new();
        env.insert("PROMPTFORGE_GATEWAY_API_KEY", "test-secret");

        let config = Config::from_env_lookup(|name| env.get(name).map(|v| (*v).to_string()))
            .expect("env config with defaults");
        assert_eq!(config.gateway.api_key, "test-secret");
        assert_eq!(config.gateway.base_url, "http://127.0.0.1:8081");
        assert_eq!(config.tape.path, PathBuf::from("tape.jsonl"));
        assert_eq!(config.server.bind, "127.0.0.1:7910");
        assert!(!config.server.open_browser);
        assert!(!config.voice.enabled());
        assert_eq!(config.voice.window_seconds, DEFAULT_VOICE_WINDOW_SECONDS);
        assert_eq!(config.voice.interval_ms, DEFAULT_VOICE_INTERVAL_MS);
    }

    #[test]
    fn from_env_errors_when_api_key_missing() {
        let env: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        let err = Config::from_env_lookup(|name| env.get(name).map(|v| (*v).to_string()))
            .expect_err("missing key must fail");
        assert!(
            matches!(err, ConfigError::MissingEnvVar { ref name } if name == "PROMPTFORGE_GATEWAY_API_KEY"),
            "expected MissingEnvVar, got {err:?}"
        );
        assert!(
            err.to_string().contains("PROMPTFORGE_GATEWAY_API_KEY"),
            "error names the variable: {err}"
        );
    }

    #[test]
    fn from_env_respects_all_overrides() {
        use std::collections::HashMap;
        let mut env: HashMap<&str, &str> = HashMap::new();
        env.insert("PROMPTFORGE_GATEWAY_API_KEY", "k");
        env.insert("PROMPTFORGE_GATEWAY_BASE_URL", "http://gw:9999");
        env.insert("PROMPTFORGE_TAPE_PATH", "custom.jsonl");
        env.insert("PROMPTFORGE_SERVER_BIND", "0.0.0.0:8080");
        env.insert("PROMPTFORGE_SERVER_OPEN_BROWSER", "1");
        env.insert("PROMPTFORGE_VOICE_INTERIM_MODEL", "m1.bin");
        env.insert("PROMPTFORGE_VOICE_FINAL_MODEL", "m2.bin");
        env.insert("PROMPTFORGE_VOICE_WINDOW_SECONDS", "10");
        env.insert("PROMPTFORGE_VOICE_INTERVAL_MS", "400");

        let config = Config::from_env_lookup(|name| env.get(name).map(|v| (*v).to_string()))
            .expect("env config with all overrides");
        assert_eq!(config.gateway.base_url, "http://gw:9999");
        assert_eq!(config.tape.path, PathBuf::from("custom.jsonl"));
        assert_eq!(config.server.bind, "0.0.0.0:8080");
        assert!(config.server.open_browser);
        assert!(config.voice.enabled());
        assert_eq!(config.voice.interim_model, PathBuf::from("m1.bin"));
        assert_eq!(config.voice.final_model, PathBuf::from("m2.bin"));
        assert_eq!(config.voice.window_seconds, 10);
        assert_eq!(config.voice.interval_ms, 400);
    }
}
