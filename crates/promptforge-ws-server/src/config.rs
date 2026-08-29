//! `workshop.toml` loading: TOML parsing, `${VAR}` environment
//! interpolation, and defaults.
//!
//! Interpolation follows the promptforge convention (gateway-config CFG-007):
//! the TOML is parsed first and only string *values* are interpolated, so
//! `${VAR}` inside comments or keys is never expanded and an interpolated
//! value containing a quote, backslash, or newline cannot corrupt the
//! document. `$$` is a literal `$`. An unset variable interpolates to the
//! empty string, so the generated config's `${PROMPTFORGE_GATEWAY_URL}` and
//! `${PROMPTFORGE_GATEWAY_API_KEY}` degrade to the built-in defaults instead
//! of failing startup.

use std::path::{Path, PathBuf};

/// Path [`Config::load`] reads when no override is given.
pub const DEFAULT_CONFIG_PATH: &str = "workshop.toml";

/// Gateway base URL used when `gateway.base_url` interpolates to an empty
/// string, for example because `PROMPTFORGE_GATEWAY_URL` is unset.
pub const DEFAULT_GATEWAY_BASE_URL: &str = "http://127.0.0.1:8081";

/// Workshop server configuration loaded from `workshop.toml`.
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
    /// let config = promptforge_ws_server::Config::from_toml_str(
    ///     "[gateway]\nbase_url = \"http://127.0.0.1:8081\"\napi_key = \"k\"\n",
    /// )?;
    /// assert_eq!(config.server.bind, "127.0.0.1:7910");
    /// # Ok::<(), promptforge_ws_server::ConfigError>(())
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
        if config.gateway.base_url.is_empty() {
            config.gateway.base_url = DEFAULT_GATEWAY_BASE_URL.to_string();
        }
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
    /// Address the workshop server binds to.
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
pub const DEFAULT_VOICE_WINDOW_SECONDS: u64 = 15;

/// Default interval between interim transcriptions, in milliseconds.
pub const DEFAULT_VOICE_INTERVAL_MS: u64 = 500;

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
    /// URL the interim model can be downloaded from. Informational until
    /// the gateway cache integration lands; empty means no known source.
    pub interim_source: String,
    /// URL the final-pass model can be downloaded from. Informational
    /// until the gateway cache integration lands; empty means no known
    /// source.
    pub final_source: String,
    /// Seconds of trailing audio each interim pass transcribes.
    pub window_seconds: u64,
    /// Milliseconds between interim passes while a take is recording.
    pub interval_ms: u64,
    /// Domain terms whisper is biased toward (for example `MCP`, `GGUF`,
    /// `Lua`), formatted into a glossary conditioning prompt on both
    /// workers. Empty disables biasing.
    pub vocabulary: Vec<String>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            interim_model: PathBuf::new(),
            final_model: PathBuf::new(),
            interim_source: String::new(),
            final_source: String::new(),
            window_seconds: DEFAULT_VOICE_WINDOW_SECONDS,
            interval_ms: DEFAULT_VOICE_INTERVAL_MS,
            vocabulary: Vec::new(),
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

impl From<&VoiceConfig> for promptforge_transcribe::EngineConfig {
    // The narrow seam into the transcription engine: plain values only, so
    // the engine crate never names this server's configuration types. An
    // empty `final_model` becomes `None`, which disables the final pass.
    fn from(config: &VoiceConfig) -> Self {
        Self {
            interim_model: config.interim_model.clone(),
            final_model: (!config.final_model.as_os_str().is_empty())
                .then(|| config.final_model.clone()),
            vocabulary: config.vocabulary.clone(),
            window_seconds: config.window_seconds,
            interval_ms: config.interval_ms,
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
        assert!(config.voice.interim_source.is_empty());
        assert!(config.voice.final_source.is_empty());
        assert_eq!(config.voice.window_seconds, DEFAULT_VOICE_WINDOW_SECONDS);
        assert_eq!(config.voice.interval_ms, DEFAULT_VOICE_INTERVAL_MS);
        assert!(config.voice.vocabulary.is_empty());
    }

    #[test]
    fn voice_section_parses_model_source_urls() {
        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"

[voice]
interim_source = "https://example.com/models/ggml-large-v3-turbo.bin"
final_source = "https://example.com/models/ggml-large-v3.bin"
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert!(!config.voice.enabled(), "sources alone do not enable voice");
        assert_eq!(
            config.voice.interim_source,
            "https://example.com/models/ggml-large-v3-turbo.bin"
        );
        assert_eq!(
            config.voice.final_source,
            "https://example.com/models/ggml-large-v3.bin"
        );
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
    fn voice_section_parses_vocabulary() {
        let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"

[voice]
vocabulary = ["MCP", "GGUF", "Lua"]
"#;
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert_eq!(config.voice.vocabulary, ["MCP", "GGUF", "Lua"]);
    }

    #[test]
    fn voice_config_maps_into_engine_config() {
        let voice = VoiceConfig {
            interim_model: PathBuf::from("models/interim.bin"),
            final_model: PathBuf::from("models/final.bin"),
            vocabulary: vec!["MCP".to_string(), "GGUF".to_string()],
            window_seconds: 8,
            interval_ms: 400,
            ..VoiceConfig::default()
        };
        let engine = promptforge_transcribe::EngineConfig::from(&voice);
        assert_eq!(engine.interim_model, PathBuf::from("models/interim.bin"));
        assert_eq!(engine.final_model, Some(PathBuf::from("models/final.bin")));
        assert_eq!(engine.vocabulary, ["MCP", "GGUF"]);
        assert_eq!(engine.window_seconds, 8);
        assert_eq!(engine.interval_ms, 400);

        let no_final = promptforge_transcribe::EngineConfig::from(&VoiceConfig::default());
        assert_eq!(
            no_final.final_model, None,
            "an empty final_model disables the final pass instead of \
             becoming a path the engine would try to load"
        );
    }

    #[test]
    fn double_dollar_is_literal() {
        let raw = "[gateway]\nbase_url = \"http://x\"\napi_key = \"cost $$5\"\n";
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert_eq!(config.gateway.api_key, "cost $5");
    }

    #[test]
    fn unset_variable_interpolates_to_empty() {
        let raw =
            "[gateway]\nbase_url = \"http://x\"\napi_key = \"${PFG_WB_DEFINITELY_UNSET_XYZ}\"\n";
        let config = Config::from_toml_str(raw).expect("unset variable resolves to empty");
        assert_eq!(config.gateway.api_key, "");
    }

    #[test]
    fn empty_base_url_falls_back_to_the_default() {
        let raw = "[gateway]\nbase_url = \"${PFG_WB_DEFINITELY_UNSET_XYZ}\"\napi_key = \"k\"\n";
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert_eq!(config.gateway.base_url, DEFAULT_GATEWAY_BASE_URL);
    }

    #[test]
    fn explicit_base_url_is_kept() {
        let raw = "[gateway]\nbase_url = \"http://gw:9999\"\napi_key = \"k\"\n";
        let config = Config::from_toml_str(raw).expect("fixture parses");
        assert_eq!(config.gateway.base_url, "http://gw:9999");
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
        let err = Config::load(Path::new("definitely-missing-workshop.toml"))
            .expect_err("missing file must fail");
        assert!(
            matches!(err, ConfigError::NotFound { .. }),
            "expected NotFound, got {err:?}"
        );
        assert!(
            err.to_string().contains("definitely-missing-workshop.toml"),
            "error names the path: {err}"
        );
    }

    #[test]
    fn unreadable_existing_file_is_a_read_error() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("workshop.toml");
        std::fs::write(&path, "[gateway]\n").expect("write fixture");
        // A directory-shaped read failure: replace the file with a
        // directory of the same name so the read fails for a reason other
        // than NotFound.
        std::fs::remove_file(&path).expect("remove fixture");
        std::fs::create_dir(&path).expect("directory in the file's place");
        let err = Config::load(&path).expect_err("unreadable path must fail");
        assert!(
            matches!(err, ConfigError::Read { .. }),
            "expected Read, got {err:?}"
        );
    }

    #[test]
    fn parse_error_names_the_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("broken-workshop.toml");
        std::fs::write(&path, "[gateway\n").expect("write fixture");
        let err = Config::load(&path).expect_err("malformed TOML must fail");
        assert!(
            matches!(err, ConfigError::Parse { .. }),
            "expected Parse, got {err:?}"
        );
        assert!(
            err.to_string().contains("broken-workshop.toml"),
            "error names the path: {err}"
        );
    }

    #[test]
    fn load_reads_and_parses_a_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("workshop.toml");
        std::fs::write(
            &path,
            "[gateway]\nbase_url = \"http://127.0.0.1:8081\"\napi_key = \"k\"\n",
        )
        .expect("write fixture");
        let config = Config::load(&path).expect("fixture loads");
        assert_eq!(config.gateway.api_key, "k");
    }
}
