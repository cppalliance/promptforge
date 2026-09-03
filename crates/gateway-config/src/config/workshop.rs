//! The optional `[workshop]` section: the embedded workshop UI server the
//! gateway can host on a second loopback listener.
//!
//! There is deliberately no `[workshop.gateway]` sub-table: the hosting
//! gateway derives the workshop's client URL from its own
//! `[server]` bind ([`ServerConfig::client_url`](super::ServerConfig::client_url))
//! and reuses the same api_key, so no credential is duplicated and none can
//! drift.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default sliding-window length for interim transcription, in seconds.
/// Mirrors the workshop server's own default.
const DEFAULT_STT_WINDOW_SECONDS: u64 = 15;

/// Default interval between interim transcriptions, in milliseconds.
/// Mirrors the workshop server's own default.
const DEFAULT_STT_INTERVAL_MS: u64 = 500;

/// The tape filename used when `[workshop.tape]` does not name one.
/// Mirrors the workshop server's own default.
const DEFAULT_TAPE_FILE: &str = "tape.jsonl";

fn default_workshop_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 7910))
}

/// The `[workshop]` section: settings for the workshop UI server hosted by
/// the gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkshopConfig {
    /// The socket address the workshop listener binds. Defaults to
    /// `127.0.0.1:7910`.
    #[serde(default = "default_workshop_bind")]
    bind: SocketAddr,
    /// Whether the gateway opens the system browser at the workshop URL once
    /// it is serving. Defaults to false.
    #[serde(default)]
    open_browser: bool,
    /// Speech-to-text capture settings. Absent when no `[workshop.stt]`
    /// section is present.
    #[serde(default)]
    stt: Option<WorkshopSttConfig>,
    /// Session tape settings. Absent when no `[workshop.tape]` section is
    /// present.
    #[serde(default)]
    tape: Option<WorkshopTapeConfig>,
}

impl WorkshopConfig {
    /// Returns the socket address the workshop listener binds.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let workshop = config.workshop().expect("workshop section present");
    /// assert_eq!(workshop.bind().to_string(), "127.0.0.1:7910");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// Returns whether the gateway opens the system browser at the workshop
    /// URL once it is serving.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop]
    /// # open_browser = true
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.workshop().expect("workshop section present").open_browser());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn open_browser(&self) -> bool {
        self.open_browser
    }

    /// Returns the `[workshop.stt]` settings, or `None` when the section
    /// is absent.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop.stt]
    /// # window_seconds = 8
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let workshop = config.workshop().expect("workshop section present");
    /// assert!(workshop.stt().is_some());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn stt(&self) -> Option<&WorkshopSttConfig> {
        self.stt.as_ref()
    }

    /// Returns the `[workshop.tape]` settings, or `None` when the section is
    /// absent.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop.tape]
    /// # path = "session.jsonl"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let workshop = config.workshop().expect("workshop section present");
    /// assert!(workshop.tape().is_some());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn tape(&self) -> Option<&WorkshopTapeConfig> {
        self.tape.as_ref()
    }

    /// Returns the tape file path anchored against `boot_dir`, the directory
    /// holding the boot config.
    ///
    /// An absent `[workshop.tape]` (or an absent path) resolves the default
    /// `tape.jsonl` against `boot_dir`; a relative path resolves against
    /// `boot_dir`; an absolute path is returned unchanged. The process
    /// current directory never participates, so the tape file (and the
    /// workshop state persisted beside it) cannot scatter with the embedding
    /// binary's start directory.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # use std::path::{Path, PathBuf};
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let workshop = config.workshop().expect("workshop section present");
    /// assert_eq!(
    ///     workshop.tape_path(Path::new("/etc/pf")),
    ///     PathBuf::from("/etc/pf").join("tape.jsonl")
    /// );
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn tape_path(&self, boot_dir: &Path) -> PathBuf {
        let path = self.tape.as_ref().map_or_else(
            || PathBuf::from(DEFAULT_TAPE_FILE),
            |tape| tape.path.clone(),
        );
        if path.is_absolute() {
            path
        } else {
            boot_dir.join(path)
        }
    }
}

/// The `[workshop.stt]` section: speech capture window, cadence, and bias.
///
/// Model sources and roles live in global `[[stt_model]]` catalog entries and
/// profiles enable them through membership.
///
/// # Examples
/// ```
/// use gateway_config::Config;
///
/// let config = Config::from_toml_str(
///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
///      [workshop.stt]\nwindow_seconds = 8\n",
/// )?;
/// assert_eq!(
///     config.workshop().and_then(|workshop| workshop.stt()).map(|stt| stt.window_seconds()),
///     Some(8)
/// );
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkshopSttConfig {
    /// Seconds of trailing audio each interim pass transcribes.
    window_seconds: u64,
    /// Milliseconds between interim passes while a take is recording.
    interval_ms: u64,
    /// Domain terms whisper is biased toward. Empty disables biasing.
    vocabulary: Vec<String>,
}

impl Default for WorkshopSttConfig {
    fn default() -> WorkshopSttConfig {
        WorkshopSttConfig {
            window_seconds: DEFAULT_STT_WINDOW_SECONDS,
            interval_ms: DEFAULT_STT_INTERVAL_MS,
            vocabulary: Vec::new(),
        }
    }
}

impl WorkshopSttConfig {
    /// Returns the seconds of trailing audio each interim pass transcribes.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop.stt]
    /// # window_seconds = 8
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let stt = config.workshop().and_then(|w| w.stt()).expect("stt present");
    /// assert_eq!(stt.window_seconds(), 8);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn window_seconds(&self) -> u64 {
        self.window_seconds
    }

    /// Returns the milliseconds between interim passes while a take is
    /// recording.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop.stt]
    /// # interval_ms = 250
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let stt = config.workshop().and_then(|w| w.stt()).expect("stt present");
    /// assert_eq!(stt.interval_ms(), 250);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    /// Returns the domain terms whisper is biased toward.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop.stt]
    /// # vocabulary = ["MCP", "GGUF"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let stt = config.workshop().and_then(|w| w.stt()).expect("stt present");
    /// assert_eq!(stt.vocabulary(), ["MCP", "GGUF"]);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn vocabulary(&self) -> &[String] {
        &self.vocabulary
    }
}

/// The `[workshop.tape]` section: session tape settings, mirroring the
/// workshop server's own tape settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkshopTapeConfig {
    /// Path of the JSONL tape file.
    path: PathBuf,
}

impl Default for WorkshopTapeConfig {
    fn default() -> WorkshopTapeConfig {
        WorkshopTapeConfig {
            path: PathBuf::from(DEFAULT_TAPE_FILE),
        }
    }
}

impl WorkshopTapeConfig {
    /// Returns the configured tape file path, exactly as written.
    ///
    /// Anchor it with [`WorkshopConfig::tape_path`] before use: a relative
    /// value resolves against the boot-config directory, never the process
    /// current directory.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # use std::path::Path;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [workshop.tape]
    /// # path = "session.jsonl"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let tape = config.workshop().and_then(|w| w.tape()).expect("tape present");
    /// assert_eq!(tape.path(), Path::new("session.jsonl"));
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::WorkshopConfig;
    use crate::config::Config;

    /// A minimal valid `[server]` to prefix workshop fixtures with.
    const BASE: &str = "config-version = 2\n[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"k\"\n";

    fn parse(extra: &str) -> Config {
        Config::from_toml_str(&format!("{BASE}{extra}")).expect("fixture parses")
    }

    #[test]
    fn workshop_absent_is_none() {
        assert!(parse("").workshop().is_none());
    }

    #[test]
    fn workshop_empty_section_takes_defaults() {
        let config = parse("[workshop]\n");
        let workshop = config.workshop().expect("workshop section present");
        assert_eq!(workshop.bind().to_string(), "127.0.0.1:7910");
        assert!(!workshop.open_browser());
        assert!(workshop.stt().is_none());
        assert!(workshop.tape().is_none());
    }

    #[test]
    fn workshop_section_parses_explicit_fields() {
        let config = parse(
            r#"
[workshop]
bind = "127.0.0.1:7999"
open_browser = true

[workshop.stt]
window_seconds = 8
interval_ms = 250
vocabulary = ["MCP", "GGUF"]

[workshop.tape]
path = "session.jsonl"
"#,
        );
        let workshop = config.workshop().expect("workshop section present");
        assert_eq!(workshop.bind().to_string(), "127.0.0.1:7999");
        assert!(workshop.open_browser());
        let stt = workshop.stt().expect("stt present");
        assert_eq!(stt.window_seconds(), 8);
        assert_eq!(stt.interval_ms(), 250);
        assert_eq!(stt.vocabulary(), ["MCP", "GGUF"]);
        let tape = workshop.tape().expect("tape present");
        assert_eq!(tape.path(), Path::new("session.jsonl"));
    }

    #[test]
    fn workshop_stt_defaults_match_capture_defaults() {
        let config = parse("[workshop.stt]\n");
        let stt = config
            .workshop()
            .and_then(WorkshopConfig::stt)
            .expect("stt present");
        assert_eq!(stt.window_seconds(), 15);
        assert_eq!(stt.interval_ms(), 500);
        assert!(stt.vocabulary().is_empty());
    }

    #[test]
    fn workshop_rejects_unknown_fields_in_every_sub_table() {
        for section in [
            "[workshop]\nbogus = 1\n",
            "[workshop.stt]\nbogus = 1\n",
            "[workshop.tape]\nbogus = 1\n",
        ] {
            let error = Config::from_toml_str(&format!("{BASE}{section}"))
                .expect_err("an unknown workshop field must fail");
            assert_eq!(error.kind(), crate::ConfigErrorKind::Parse, "in {section}");
        }
    }

    #[test]
    fn workshop_client_url_swaps_unspecified_bind_for_loopback() {
        let url = |bind: &str| {
            Config::from_toml_str(&format!(
                "config-version = 2\n[server]\nbind = \"{bind}\"\napi_key = \"k\"\n"
            ))
            .expect("fixture parses")
            .server()
            .client_url()
        };
        assert_eq!(url("0.0.0.0:8081"), "http://127.0.0.1:8081");
        assert_eq!(url("[::]:8081"), "http://[::1]:8081");
    }

    #[test]
    fn workshop_client_url_keeps_reachable_binds() {
        let url = |bind: &str| {
            Config::from_toml_str(&format!(
                "config-version = 2\n[server]\nbind = \"{bind}\"\napi_key = \"k\"\n"
            ))
            .expect("fixture parses")
            .server()
            .client_url()
        };
        assert_eq!(url("127.0.0.1:8081"), "http://127.0.0.1:8081");
        assert_eq!(url("192.168.1.5:9000"), "http://192.168.1.5:9000");
    }

    #[test]
    fn workshop_tape_path_anchors_absent_and_relative_against_the_boot_dir() {
        let boot_dir = Path::new("boot-dir");

        let absent = parse("[workshop]\n");
        assert_eq!(
            absent.workshop().expect("present").tape_path(boot_dir),
            boot_dir.join("tape.jsonl"),
            "an absent [workshop.tape] anchors the default filename"
        );

        let relative = parse("[workshop.tape]\npath = \"tapes/session.jsonl\"\n");
        assert_eq!(
            relative.workshop().expect("present").tape_path(boot_dir),
            boot_dir.join("tapes").join("session.jsonl"),
            "a relative path anchors against the boot dir, not the cwd"
        );
    }

    #[test]
    fn workshop_tape_path_keeps_an_absolute_path() {
        let absolute = std::env::temp_dir().join("pf-tape.jsonl");
        let config = parse(&format!(
            "[workshop.tape]\npath = {:?}\n",
            absolute.display().to_string()
        ));
        assert_eq!(
            config
                .workshop()
                .expect("present")
                .tape_path(Path::new("boot-dir")),
            PathBuf::from(&absolute)
        );
    }

    #[test]
    fn workshop_config_round_trips_through_json() {
        let config = parse(
            r#"
[workshop]
bind = "127.0.0.1:7999"
open_browser = true

[workshop.stt]
window_seconds = 8
vocabulary = ["MCP", "GGUF"]

[workshop.tape]
path = "session.jsonl"
"#,
        );
        let workshop = config.workshop().expect("workshop section present");
        let json = serde_json::to_value(workshop).expect("serializes");
        let back: WorkshopConfig = serde_json::from_value(json).expect("deserializes");
        assert_eq!(workshop, &back);
    }
}
