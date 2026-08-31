//! `impl Config`: public load/parse entry points and read-only accessors.
//!
//! Semantic validation lives in [`super::validate`]; `${VAR}` interpolation
//! lives in [`super::interpolate`]. This module is only the loading seam and the
//! accessors the rest of the crate reads a validated `Config` through.

use std::fs;
use std::net::SocketAddr;
use std::ops::Range;
use std::path::Path;

use serde::Deserialize;

use super::{Config, RawConfig, Secret, WebSearchConfig, interpolate_value};
use crate::error::ConfigError;
use crate::profile::{ProfileName, ProfileSelection, resolve_selection};

impl Config {
    /// Loads one version-2 configuration file and selects its startup profile.
    ///
    /// `${VAR}` interpolation reads the process environment as the caller left
    /// it. The caller supplies command-line and environment profile values in
    /// [`ProfileSelection`]; those values outrank the sibling state file.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) when the file or state
    /// cannot be read, the TOML or interpolation is invalid, a removed layout
    /// feature is present, no profile is selected, or semantic validation
    /// fails.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_gateway_config::{Config, ProfileSelection};
    /// use std::path::Path;
    ///
    /// let inputs = ProfileSelection::new(Some("work"), None);
    /// let config = Config::load(Path::new("gateway.toml"), &inputs)?;
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    pub fn load(
        path: &Path,
        inputs: &ProfileSelection,
    ) -> Result<Config, crate::api_error::ConfigError> {
        Self::load_repr(path, inputs).map_err(crate::api_error::ConfigError::from)
    }

    fn load_repr(path: &Path, inputs: &ProfileSelection) -> Result<Config, ConfigError> {
        reject_profiles_directory(path)?;
        let raw = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let mut config = Self::parse_toml_at(&raw, Some(path))?;
        let Some(selected) = resolve_selection(path, inputs)? else {
            return Err(ConfigError::Validation(format!(
                "no active profile selected; define one with --profile, \
                 PROMPTFORGE_PROFILE, or {} (defined profiles: {})",
                crate::profile_state_path(path).display(),
                config.defined_profile_names()
            )));
        };
        config.activate_profile(&selected)?;
        Ok(config)
    }

    /// The server bind address.
    #[must_use]
    pub fn bind_addr(&self) -> SocketAddr {
        self.server.bind
    }

    /// A clone of the server's bearer key.
    #[must_use]
    pub fn server_key(&self) -> Secret {
        self.server.api_key.clone()
    }

    /// The web-search configuration, when `[tools.web_search]` is present.
    #[must_use]
    pub fn web_search_config(&self) -> Option<&WebSearchConfig> {
        self.tools
            .as_ref()
            .and_then(|tools| tools.web_search.as_ref())
    }

    /// Reconstructs the global TOML shape, independent of active selection.
    pub(crate) fn to_raw(&self) -> RawConfig {
        RawConfig {
            config_version: self.version,
            server: self.server.clone(),
            local: self.local.clone(),
            dominions: self.dominions.clone(),
            endpoints: self.endpoints.clone(),
            models: self.catalog_models.clone(),
            local_models: self.catalog_local_models.clone(),
            stt_models: self.catalog_stt_models.clone(),
            profiles: self.profiles.clone(),
            tools: self.tools.clone(),
            workshop: self.workshop.clone(),
        }
    }

    /// Serializes the global configuration as a JSON object in TOML shape.
    ///
    /// The payload mirrors the operator-authored global catalog and profiles.
    /// Active selection remains sibling state and is available through
    /// [`Config::active_profile`]. Secret fields render as `"***"`.
    ///
    /// # Panics
    /// Panics if the raw shape fails to serialize. Its serializers are
    /// infallible (plain data plus the redacting `Secret` marker), so a
    /// failure is a schema bug, not operator input.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// let toml = r#"
    /// config-version = 2
    /// [server]
    /// bind = "127.0.0.1:8080"
    /// api_key = "secret"
    /// "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.to_json()["server"]["api_key"], "***");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self.to_raw()).unwrap_or_else(|error| {
            // The raw shape is plain data whose serializers cannot fail; a
            // failure here is a schema bug, not operator input.
            panic!("config serialization to JSON failed: {error}");
        })
    }

    /// Interpolate, parse, and validate a configuration from a TOML string.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) for a malformed or unresolved
    /// interpolation, invalid TOML, or a failed semantic check.
    ///
    /// # Examples
    /// ```
    /// use promptforge_gateway_config::Config;
    ///
    /// let toml = r#"
    /// config-version = 2
    /// [server]
    /// bind = "127.0.0.1:8080"
    /// api_key = "secret"
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
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].name(), "m");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    pub fn from_toml_str(raw: &str) -> Result<Config, crate::api_error::ConfigError> {
        Self::parse_toml(raw).map_err(crate::api_error::ConfigError::from)
    }

    /// Returns a clone with `name` selected from the already-loaded catalog.
    ///
    /// This operation performs no file or environment read. Every profile was
    /// validated when the catalog loaded, so selection only derives the three
    /// active model subsets.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) when `name` is not defined.
    ///
    /// # Examples
    /// ```
    /// use promptforge_gateway_config::{Config, ProfileName};
    ///
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n\
    ///      [server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    ///      [[profile]]\nname = \"work\"\nmodels = []\n",
    /// )?;
    /// let selected = config.select_profile(&ProfileName::parse("work")?)?;
    /// assert_eq!(selected.active_profile().map(|profile| profile.name()), Some("work"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn select_profile(
        &self,
        name: &ProfileName,
    ) -> Result<Config, crate::api_error::ConfigError> {
        let mut selected = self.clone();
        selected
            .activate_profile(name)
            .map_err(crate::api_error::ConfigError::from)?;
        Ok(selected)
    }

    /// Parse, interpolate, and validate, returning the internal error type.
    pub(crate) fn parse_toml(raw: &str) -> Result<Config, ConfigError> {
        Self::parse_toml_at(raw, None)
    }

    pub(crate) fn parse_toml_at(raw: &str, path: Option<&Path>) -> Result<Config, ConfigError> {
        // Parse first, then interpolate only string *values*. Interpolating the
        // raw text would expand `${VAR}` inside comments and keys, and an
        // interpolated value containing a quote, backslash, or newline would
        // corrupt the TOML structure on a second parse. (CFG-007)
        let document: toml::Value = toml::from_str(raw).map_err(|source| ConfigError::Parse {
            path: path.map(Path::to_path_buf),
            source: Box::new(source),
        })?;
        reject_removed_layout(raw, path)?;
        Self::from_value(document)
    }

    /// Interpolates string leaves, deserializes, and validates an already
    /// parsed TOML document.
    pub(crate) fn from_value(mut document: toml::Value) -> Result<Config, ConfigError> {
        interpolate_value(&mut document)?;
        let raw: RawConfig = document.try_into().map_err(|source| ConfigError::Parse {
            path: None,
            source: Box::new(source),
        })?;
        let mut config = Config::from(raw);
        config.imply_projector_images();
        config.validate()?;
        Ok(config)
    }
}

pub(crate) fn reject_profiles_directory(path: &Path) -> Result<(), ConfigError> {
    let profiles = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("profiles");
    if profiles.is_dir() {
        return Err(ConfigError::HardBreak {
            path: path.to_owned(),
            line: 1,
            key: "profiles/",
            replacement: "move every profile into this file as a [[profile]] checklist",
        });
    }
    Ok(())
}

#[derive(Deserialize, Default)]
struct RemovedLayoutProbe {
    #[serde(rename = "config-version", default)]
    config_version: Option<toml::Spanned<toml::Value>>,
    #[serde(default)]
    include: Option<toml::Spanned<toml::Value>>,
    #[serde(default)]
    models: Option<toml::Spanned<toml::Value>>,
    #[serde(default)]
    workshop: Option<RemovedWorkshopProbe>,
}

#[derive(Deserialize, Default)]
struct RemovedWorkshopProbe {
    #[serde(default)]
    voice: Option<RemovedVoiceProbe>,
}

#[derive(Deserialize, Default)]
struct RemovedVoiceProbe {
    #[serde(default)]
    interim_model: Option<toml::Spanned<toml::Value>>,
    #[serde(default)]
    final_model: Option<toml::Spanned<toml::Value>>,
    #[serde(default)]
    interim_source: Option<toml::Spanned<toml::Value>>,
    #[serde(default)]
    final_source: Option<toml::Spanned<toml::Value>>,
}

fn reject_removed_layout(raw: &str, path: Option<&Path>) -> Result<(), ConfigError> {
    let path = path.map_or_else(|| std::path::PathBuf::from("<memory>"), Path::to_path_buf);
    let probe: RemovedLayoutProbe = toml::from_str(raw).map_err(|source| ConfigError::Parse {
        path: Some(path.clone()),
        source: Box::new(source),
    })?;

    if let Some(value) = probe.include {
        return Err(hard_break(
            path,
            line_for_span(raw, value.span()),
            "include",
            "use one gateway.toml with [[profile]] checklist entries",
        ));
    }
    if let Some(value) = probe.models {
        return Err(hard_break(
            path,
            line_for_span(raw, value.span()),
            "models",
            "move this checklist into a [[profile]] models key",
        ));
    }
    if let Some(voice) = probe.workshop.and_then(|workshop| workshop.voice) {
        for (value, key) in [
            (voice.interim_model.as_ref(), "workshop.voice.interim_model"),
            (voice.final_model.as_ref(), "workshop.voice.final_model"),
            (
                voice.interim_source.as_ref(),
                "workshop.voice.interim_source",
            ),
            (voice.final_source.as_ref(), "workshop.voice.final_source"),
        ] {
            if let Some(value) = value {
                return Err(hard_break(
                    path,
                    line_for_span(raw, value.span()),
                    key,
                    "use [workshop.stt] tuning and a global [[stt_model]] entry",
                ));
            }
        }
        return Err(hard_break(
            path,
            find_voice_header_line(raw).unwrap_or(1),
            "workshop.voice",
            "rename capture tuning to [workshop.stt] and define models as [[stt_model]]",
        ));
    }

    match probe.config_version {
        Some(version) if version.get_ref().as_integer() == Some(2) => Ok(()),
        Some(version) => Err(hard_break(
            path,
            line_for_span(raw, version.span()),
            "config-version",
            "set config-version = 2 and use the single-file profile layout",
        )),
        None => Err(hard_break(
            path,
            1,
            "config-version",
            "add config-version = 2 before the first table",
        )),
    }
}

fn hard_break(
    path: std::path::PathBuf,
    line: usize,
    key: &'static str,
    replacement: &'static str,
) -> ConfigError {
    ConfigError::HardBreak {
        path,
        line,
        key,
        replacement,
    }
}

fn line_for_span(raw: &str, span: Range<usize>) -> usize {
    raw[..span.start.min(raw.len())].split('\n').count()
}

fn find_voice_header_line(raw: &str) -> Option<usize> {
    raw.lines().enumerate().find_map(|(index, line)| {
        let trimmed = line.trim();
        let header = trimmed
            .strip_prefix("[[")
            .and_then(|rest| rest.strip_suffix("]]"))
            .or_else(|| {
                trimmed
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'))
            })?;
        let normalized = header
            .split('.')
            .map(|segment| segment.trim().trim_matches(['"', '\'']))
            .collect::<Vec<_>>()
            .join(".");
        (normalized == "workshop.voice").then_some(index + 1)
    })
}
