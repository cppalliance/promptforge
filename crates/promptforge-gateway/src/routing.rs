//! The routing table: model name to backend endpoint.

use std::collections::HashMap;
use std::sync::Arc;

use promptforge_gateway_config::{Config, ConfigError, Protocol, ThinkingMode};

use crate::error::GatewayError;
use crate::queue::EndpointLane;
use crate::upstream::{OpenAiUpstream, Upstream};

/// One backend endpoint plus the upstream that talks to it.
pub(crate) struct Endpoint {
    /// The endpoint's configured id.
    pub id: String,
    /// The upstream implementation forwarding to this backend.
    pub upstream: Arc<dyn Upstream>,
    /// Per-endpoint admission control (concurrency + waiting queue).
    pub lane: EndpointLane,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("id", &self.id)
            .field("lane", &self.lane)
            .finish_non_exhaustive()
    }
}

/// One model, resolved to a backend endpoint and the backend's model string.
#[derive(Debug)]
pub(crate) struct Model {
    /// The caller-facing model name.
    pub name: String,
    /// Prose describing the model for catalog consumers.
    pub description: String,
    /// Context window size in tokens.
    pub context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    pub thinking: ThinkingMode,
    /// The tool-calling dialect used by this model (e.g. `"openai"`, `"gemma3_tool_code"`).
    pub tool_dialect: String,
    /// Whether tool calls are handled natively or emulated (`"native"`, `"emulated"`).
    pub tools_mode: String,
    /// The string the backend knows this model by.
    pub upstream_name: String,
    /// The endpoint serving this model (v0 uses the first configured one).
    pub endpoint: Arc<Endpoint>,
}

/// A resolved routing table.
#[derive(Debug)]
pub(crate) struct Routing {
    by_name: HashMap<String, Arc<Model>>,
    /// Configured models in `gateway.toml` order, for the catalog listing.
    models: Vec<Arc<Model>>,
}

impl Routing {
    /// Build a routing table directly from resolved models. Intended for tests
    /// and for [`Routing::from_config`]. Order of `models` is the catalog order.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] when two models share a name, which
    /// would otherwise silently shadow one entry in the lookup table while
    /// leaving both in the catalog listing.
    pub(crate) fn new(models: Vec<Arc<Model>>) -> Result<Routing, ConfigError> {
        let mut by_name = HashMap::with_capacity(models.len());
        for model in &models {
            if by_name
                .insert(model.name.clone(), Arc::clone(model))
                .is_some()
            {
                return Err(ConfigError::validation(format!(
                    "duplicate model name {}",
                    model.name
                )));
            }
        }
        Ok(Routing { by_name, models })
    }

    /// Configured models in catalog order.
    #[must_use]
    pub(crate) fn models(&self) -> &[Arc<Model>] {
        &self.models
    }

    /// Build a routing table from a validated [`Config`], constructing one
    /// upstream per endpoint.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] if a model references an endpoint
    /// that is not defined (which [`Config::validate`] already rejects, so this
    /// is a defensive second check).
    pub(crate) fn from_config(config: &Config) -> Result<Routing, ConfigError> {
        let mut endpoints: HashMap<&str, Arc<Endpoint>> = HashMap::new();
        for endpoint in &config.endpoints {
            let upstream: Arc<dyn Upstream> = match endpoint.protocol {
                Protocol::Openai => Arc::new(OpenAiUpstream::new(
                    &endpoint.base_url,
                    endpoint.api_key.clone(),
                )),
            };
            let lane = match config.endpoint_concurrency(endpoint) {
                Some(n) => EndpointLane::new(n, &config.queue),
                None => EndpointLane::unlimited(),
            };
            endpoints.insert(
                endpoint.id.as_str(),
                Arc::new(Endpoint {
                    id: endpoint.id.clone(),
                    upstream,
                    lane,
                }),
            );
        }

        let mut models = Vec::with_capacity(config.models.len());
        for model in &config.models {
            let endpoint_id = model.endpoints.first().ok_or_else(|| {
                ConfigError::validation(format!("model {} has no endpoints", model.name))
            })?;
            let endpoint = endpoints.get(endpoint_id.as_str()).ok_or_else(|| {
                ConfigError::validation(format!(
                    "model {} names undefined endpoint {endpoint_id}",
                    model.name
                ))
            })?;
            models.push(Arc::new(Model {
                name: model.name.clone(),
                description: model.description.clone(),
                context: model.context,
                thinking: model.thinking,
                tool_dialect: "openai".to_owned(),
                tools_mode: "native".to_owned(),
                upstream_name: model.upstream.clone(),
                endpoint: Arc::clone(endpoint),
            }));
        }

        Routing::new(models)
    }

    /// Appends models (for example from [`crate::local::LocalRuntime`]) to this table.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] when a model name already exists.
    pub(crate) fn merge(
        mut self,
        extras: impl IntoIterator<Item = Arc<Model>>,
    ) -> Result<Routing, ConfigError> {
        for model in extras {
            if self.by_name.contains_key(&model.name) {
                return Err(ConfigError::validation(format!(
                    "duplicate model name {}",
                    model.name
                )));
            }
            self.by_name.insert(model.name.clone(), Arc::clone(&model));
            self.models.push(model);
        }
        Ok(self)
    }

    /// Resolve a model name to its routing entry.
    ///
    /// # Errors
    /// Returns [`GatewayError::UnknownModel`] when no `[[model]]` matches.
    pub(crate) fn model(&self, name: &str) -> Result<Arc<Model>, GatewayError> {
        self.by_name
            .get(name)
            .cloned()
            .ok_or_else(|| GatewayError::UnknownModel(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use promptforge_gateway_config::{ConfigErrorKind, Secret};

    fn model_named(name: &str) -> Arc<Model> {
        let endpoint = Arc::new(Endpoint {
            id: "e".to_owned(),
            upstream: Arc::new(OpenAiUpstream::new(
                "http://127.0.0.1:9",
                Secret::new(String::new()),
            )),
            lane: EndpointLane::unlimited(),
        });
        Arc::new(Model {
            name: name.to_owned(),
            description: "d".to_owned(),
            context: 8192,
            thinking: ThinkingMode::Never,
            tool_dialect: "openai".to_owned(),
            tools_mode: "native".to_owned(),
            upstream_name: "u".to_owned(),
            endpoint,
        })
    }

    fn routing() -> Routing {
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "known"
description = "a known test model"
context = 8192
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

    #[test]
    fn new_rejects_duplicate_model_names() {
        let dup = Routing::new(vec![model_named("m"), model_named("m")]);
        assert!(matches!(
            dup,
            Err(e) if e.kind() == ConfigErrorKind::Validation
        ));
    }

    #[test]
    fn new_preserves_catalog_order() {
        let r = Routing::new(vec![model_named("a"), model_named("b"), model_named("c")])
            .expect("distinct names");
        let names: Vec<&str> = r.models().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn merge_rejects_duplicate_model_names() {
        let base = Routing::new(vec![model_named("known")]).expect("distinct");
        let merged = base.merge([model_named("known")]);
        assert!(matches!(
            merged,
            Err(e) if e.kind() == ConfigErrorKind::Validation
        ));
    }

    #[test]
    fn merge_appends_after_existing_models() {
        let base = Routing::new(vec![model_named("a")]).expect("distinct");
        let merged = base.merge([model_named("b")]).expect("distinct extra");
        let names: Vec<&str> = merged.models().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }
}
