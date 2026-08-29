//! `impl Config`: public load/parse entry points and read-only accessors.
//!
//! Semantic validation lives in [`super::validate`]; `${VAR}` interpolation
//! lives in [`super::interpolate`]. This module is only the loading seam and the
//! accessors the rest of the crate reads a validated `Config` through.

use std::net::SocketAddr;
use std::path::Path;

use super::{Config, RawConfig, Secret, WebSearchConfig, interpolate_value};
use crate::error::ConfigError;

impl Config {
    /// Load a configuration file with recursive `include` resolution.
    ///
    /// `${VAR}` interpolation reads the process environment as the caller left
    /// it; this crate never populates it from env files.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) if the file cannot be read,
    /// an include cycle or depth limit is hit, an interpolation is malformed or
    /// references an unset variable, the TOML is invalid, or a semantic check
    /// fails.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_gateway_config::Config;
    /// use std::path::Path;
    ///
    /// let config = Config::load(Path::new("gateway.toml"))?;
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    pub fn load(path: &Path) -> Result<Config, crate::api_error::ConfigError> {
        crate::profile::load_path(path).map_err(crate::api_error::ConfigError::from)
    }

    /// Load a named profile from `dir` with recursive `include` resolution.
    ///
    /// `${VAR}` interpolation reads the process environment as the caller left
    /// it; this crate never populates it from env files.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) when the profile is missing,
    /// includes cycle or exceed depth, or the resolved document fails config
    /// validation.
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_gateway_config::{Config, ProfileName};
    /// use std::path::Path;
    ///
    /// let name = ProfileName::parse("dev")?;
    /// let config = Config::load_profile(Path::new("/etc/promptforge/profiles"), &name)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn load_profile(
        dir: &Path,
        name: &crate::profile::ProfileName,
    ) -> Result<Config, crate::api_error::ConfigError> {
        crate::profile::load_named(dir, name).map_err(crate::api_error::ConfigError::from)
    }

    /// Load a named profile like [`Config::load_profile`], additionally
    /// returning the resolved include chain (the profile itself first, then
    /// included files depth-first).
    ///
    /// The chain lets a caller log exactly which files produced the config and
    /// check whether another file (for example the boot config) appears in it.
    ///
    /// # Errors
    /// Returns [`ConfigError`](crate::ConfigError) under the same conditions
    /// as [`Config::load_profile`].
    ///
    /// # Examples
    /// ```no_run
    /// use promptforge_gateway_config::{Config, ProfileName};
    /// use std::path::Path;
    ///
    /// let name = ProfileName::parse("dev")?;
    /// let (config, chain) =
    ///     Config::load_profile_with_chain(Path::new("/etc/promptforge/profiles"), &name)?;
    /// println!("resolved through {} files", chain.len());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn load_profile_with_chain(
        dir: &Path,
        name: &crate::profile::ProfileName,
    ) -> Result<(Config, Vec<std::path::PathBuf>), crate::api_error::ConfigError> {
        crate::profile::load_named_with_chain(dir, name)
            .map_err(crate::api_error::ConfigError::from)
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

    /// Parse, interpolate, and validate, returning the internal error type.
    pub(crate) fn parse_toml(raw: &str) -> Result<Config, ConfigError> {
        // Parse first, then interpolate only string *values*. Interpolating the
        // raw text would expand `${VAR}` inside comments and keys, and an
        // interpolated value containing a quote, backslash, or newline would
        // corrupt the TOML structure on a second parse. (CFG-007)
        let document: toml::Value = toml::from_str(raw).map_err(|source| ConfigError::Parse {
            path: None,
            source: Box::new(source),
        })?;
        Self::from_value(document)
    }

    /// Interpolate string leaves, deserialize, and validate an already-parsed
    /// TOML document. Used by [`Self::parse_toml`] and by profile include
    /// resolution, which merges into a `toml::Value` and avoids re-serializing.
    pub(crate) fn from_value(mut document: toml::Value) -> Result<Config, ConfigError> {
        interpolate_value(&mut document)?;
        let raw: RawConfig = document.try_into().map_err(|source| ConfigError::Parse {
            path: None,
            source: Box::new(source),
        })?;
        let mut config = Config::from(raw);
        config.apply_model_allowlist()?;
        config.imply_projector_images();
        config.validate()?;
        Ok(config)
    }
}
