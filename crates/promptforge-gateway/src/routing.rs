//! The routing table: model name to backend endpoint.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::Config;
use crate::error::{ConfigError, GatewayError};
use crate::upstream::{OpenAiUpstream, Upstream};

/// One backend endpoint plus the upstream that talks to it.
pub struct Endpoint {
    /// The endpoint's configured id.
    pub id: String,
    /// The upstream implementation forwarding to this backend.
    pub upstream: Arc<dyn Upstream>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// One model, resolved to a backend endpoint and the backend's model string.
#[derive(Debug)]
pub struct Model {
    /// The caller-facing model name.
    pub name: String,
    /// The string the backend knows this model by.
    pub upstream_name: String,
    /// The endpoint serving this model (v0 uses the first configured one).
    pub endpoint: Arc<Endpoint>,
}

/// A resolved routing table.
#[derive(Debug)]
pub struct Routing {
    models: HashMap<String, Arc<Model>>,
}

impl Routing {
    /// Build a routing table directly from resolved models. Intended for tests
    /// and for [`Routing::from_config`].
    #[must_use]
    pub fn new(models: HashMap<String, Arc<Model>>) -> Routing {
        Routing { models }
    }

    /// Build a routing table from a validated [`Config`], constructing one
    /// upstream per endpoint.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] if a model references an endpoint
    /// that is not defined (which [`Config::validate`] already rejects, so this
    /// is a defensive second check).
    pub fn from_config(config: &Config) -> Result<Routing, ConfigError> {
        let mut endpoints: HashMap<&str, Arc<Endpoint>> = HashMap::new();
        for endpoint in &config.endpoints {
            let upstream: Arc<dyn Upstream> = Arc::new(OpenAiUpstream::new(
                &endpoint.base_url,
                endpoint.api_key.clone(),
            ));
            endpoints.insert(
                endpoint.id.as_str(),
                Arc::new(Endpoint {
                    id: endpoint.id.clone(),
                    upstream,
                }),
            );
        }

        let mut models = HashMap::new();
        for model in &config.models {
            let endpoint_id = model.endpoints.first().ok_or_else(|| {
                ConfigError::Validation(format!("model {} has no endpoints", model.name))
            })?;
            let endpoint = endpoints.get(endpoint_id.as_str()).ok_or_else(|| {
                ConfigError::Validation(format!(
                    "model {} names undefined endpoint {endpoint_id}",
                    model.name
                ))
            })?;
            models.insert(
                model.name.clone(),
                Arc::new(Model {
                    name: model.name.clone(),
                    upstream_name: model.upstream.clone(),
                    endpoint: Arc::clone(endpoint),
                }),
            );
        }

        Ok(Routing::new(models))
    }

    /// Resolve a model name to its routing entry.
    ///
    /// # Errors
    /// Returns [`GatewayError::UnknownModel`] when no `[[model]]` matches.
    pub fn model(&self, name: &str) -> Result<Arc<Model>, GatewayError> {
        self.models
            .get(name)
            .cloned()
            .ok_or_else(|| GatewayError::UnknownModel(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn routing() -> Routing {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
token = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "known"
upstream = "backend-name"
endpoints = ["e"]
"#;
        let config = Config::from_toml_str(toml).unwrap();
        Routing::from_config(&config).unwrap()
    }

    #[test]
    fn resolves_a_known_model() {
        let r = routing();
        let m = r.model("known").unwrap();
        assert_eq!(m.upstream_name, "backend-name");
        assert_eq!(m.endpoint.id, "e");
    }

    #[test]
    fn unknown_model_errors() {
        let r = routing();
        assert!(matches!(
            r.model("nope"),
            Err(GatewayError::UnknownModel(_))
        ));
    }
}
