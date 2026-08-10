//! `impl Config`: public load/parse entry points, validation, and accessors.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;

use super::{
    Config, DeviceKind, EndpointConfig, LocalModelConfig, RawConfig, Secret, WebSearchConfig,
    interpolate, is_sha256_hex,
};
use crate::error::ConfigError;

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
        self.validate_tools()?;
        Ok(())
    }

    /// Validate `[tools.web_search]` bounds at load so downstream code never has
    /// to clamp operator input (CFG-006).
    fn validate_tools(&self) -> Result<(), ConfigError> {
        let Some(web_search) = self.web_search_config() else {
            return Ok(());
        };
        if web_search.default_count < 1 {
            return Err(ConfigError::Validation(
                "tools.web_search.default_count must be at least 1".to_string(),
            ));
        }
        if web_search.max_count < 1 {
            return Err(ConfigError::Validation(
                "tools.web_search.max_count must be at least 1".to_string(),
            ));
        }
        if web_search.default_count > web_search.max_count {
            return Err(ConfigError::Validation(
                "tools.web_search.default_count must not exceed max_count".to_string(),
            ));
        }
        if web_search.max_per_host < 1 {
            return Err(ConfigError::Validation(
                "tools.web_search.max_per_host must be at least 1".to_string(),
            ));
        }
        let base_url = web_search.base_url.trim();
        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err(ConfigError::Validation(
                "tools.web_search.base_url must be an http(s) URL".to_string(),
            ));
        }
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
            if local_model.source.starts_with("http://") {
                return Err(ConfigError::Validation(format!(
                    "local_model {} source must use https, not plaintext http",
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
