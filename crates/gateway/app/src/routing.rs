//! The routing table: model name to backend endpoint.

use std::collections::HashMap;
use std::sync::Arc;

use gateway_config::{Config, ConfigError, ModelKind, Protocol};

use crate::error::GatewayError;
use crate::queue::DominionQueue;
use crate::upstream::{OpenAiUpstream, Upstream};

// The table-entry vocabulary (`Model`, `Endpoint`) and the dominion-queue
// builder live in the routing crate, shared with the local inference crate;
// these re-exports keep every `crate::routing::*` path resolving unchanged.
pub(crate) use gateway_routing::{Endpoint, Model, dominion_queues};

/// A resolved routing table. Cloning copies the two indexes of `Arc`
/// entries, never the endpoints behind them, so a switch can publish the
/// remote table as its interim state and still merge the local models into
/// the same table at commit.
#[derive(Debug, Clone)]
pub(crate) struct Routing {
    by_name: HashMap<String, Arc<Model>>,
    /// Configured models in `gateway.toml` order, for the catalog listing.
    models: Vec<Arc<Model>>,
}

impl Routing {
    /// A copy of this table without the model `name`, for the unload
    /// command. Infallible: filtering an already-valid table cannot create
    /// duplicates.
    #[cfg(feature = "local")]
    pub(crate) fn without(&self, name: &str) -> Routing {
        Routing {
            by_name: self
                .by_name
                .iter()
                .filter(|(key, _)| key.as_str() != name)
                .map(|(key, model)| (key.clone(), Arc::clone(model)))
                .collect(),
            models: self
                .models
                .iter()
                .filter(|model| model.name != name)
                .cloned()
                .collect(),
        }
    }

    /// Builds a routing table directly from resolved models. Intended for tests
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

    /// Builds a routing table from a validated [`Config`], constructing one
    /// upstream per endpoint and one shared [`DominionQueue`] per dominion.
    ///
    /// Every endpoint bound to a dominion clones that dominion's queue, so
    /// all of them compete for one pool of concurrency slots. An endpoint
    /// without a `dominion` is an unlimited pass-through.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] if a model references an endpoint
    /// or an endpoint references a dominion that is not defined (which
    /// [`Config::validate`] already rejects, so these are defensive second
    /// checks).
    pub(crate) fn from_config(config: &Config) -> Result<Routing, ConfigError> {
        // One queue instance per dominion. Cloning a DominionQueue clones the
        // Arc-backed limit, so every bound endpoint shares the same slots.
        let dominion_queues = dominion_queues(config);

        let mut endpoints: HashMap<&str, Arc<Endpoint>> = HashMap::new();
        for endpoint in config.endpoints() {
            let upstream: Arc<dyn Upstream> = match endpoint.protocol() {
                Protocol::Openai => Arc::new(OpenAiUpstream::new(
                    endpoint.base_url(),
                    endpoint.api_key().clone(),
                )),
                _ => unreachable!("Protocol is non_exhaustive; wire up new protocols here"),
            };
            let queue = match endpoint.dominion() {
                Some(dominion_id) => dominion_queues
                    .get(dominion_id)
                    .ok_or_else(|| {
                        ConfigError::validation(format!(
                            "endpoint {} names undefined dominion {dominion_id}",
                            endpoint.id()
                        ))
                    })?
                    .clone(),
                None => DominionQueue::unlimited(),
            };
            endpoints.insert(
                endpoint.id(),
                Arc::new(Endpoint {
                    id: endpoint.id().to_owned(),
                    upstream,
                    queue,
                }),
            );
        }

        let mut models = Vec::with_capacity(config.models().len());
        for model in config.models() {
            let endpoint_id = model.endpoints().first().ok_or_else(|| {
                ConfigError::validation(format!("model {} has no endpoints", model.name()))
            })?;
            let endpoint = endpoints.get(endpoint_id.as_str()).ok_or_else(|| {
                ConfigError::validation(format!(
                    "model {} names undefined endpoint {endpoint_id}",
                    model.name()
                ))
            })?;
            models.push(Arc::new(Model {
                name: model.name().to_owned(),
                kind: model.kind(),
                description: model.description().to_owned(),
                context: model.context(),
                thinking: model.thinking(),
                capabilities: model.capabilities().clone(),
                tool_dialect: model.tool_dialect().to_string(),
                upstream_name: model.upstream().to_owned(),
                endpoint: Arc::clone(endpoint),
            }));
        }

        Routing::new(models)
    }

    /// Appends models (for example from the local crate's `LocalRuntime`) to
    /// this table.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] when a model name already exists.
    #[cfg(any(test, feature = "local"))]
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

    /// Resolves a model name to its routing entry.
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

/// Guards that a resolved model serves the workload its route handles, so a
/// request never reaches a backend wired for a different kind of work.
///
/// # Errors
/// Returns [`GatewayError::KindMismatch`] when the model's configured kind
/// differs from the kind the calling route serves.
pub(crate) fn require_kind(model: &Model, expected: ModelKind) -> Result<(), GatewayError> {
    if model.kind == expected {
        Ok(())
    } else {
        Err(GatewayError::KindMismatch {
            model: model.name.clone(),
            expected,
            actual: model.kind,
        })
    }
}

#[cfg(test)]
#[path = "routing-tests.rs"]
mod tests;
