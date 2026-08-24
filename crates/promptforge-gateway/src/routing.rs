//! The routing table: model name to backend endpoint.

use std::collections::HashMap;
use std::sync::Arc;

use promptforge_gateway_config::{
    Capabilities, Config, ConfigError, ModelKind, Protocol, ThinkingMode, ToolDialect,
};

use crate::error::GatewayError;
use crate::queue::DominionQueue;
use crate::upstream::{OpenAiUpstream, Upstream};

/// One backend endpoint plus the upstream that talks to it.
pub(crate) struct Endpoint {
    /// The endpoint's configured id.
    pub id: String,
    /// The upstream implementation forwarding to this backend.
    pub upstream: Arc<dyn Upstream>,
    /// Admission control: concurrency limit plus bounded waiting queue.
    /// Endpoints bound to the same dominion hold clones of one shared queue
    /// and compete for a single pool of slots; an endpoint with no dominion
    /// is unlimited.
    pub queue: DominionQueue,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("id", &self.id)
            .field("queue", &self.queue)
            .finish_non_exhaustive()
    }
}

/// One model, resolved to a backend endpoint and the backend's model string.
#[derive(Debug)]
pub(crate) struct Model {
    /// The caller-facing model name.
    pub name: String,
    /// The workload this model serves: chat, embedding, or classifier.
    pub kind: ModelKind,
    /// Prose describing the model for catalog consumers.
    pub description: String,
    /// Context window size in tokens.
    pub context: u32,
    /// Whether thinking tokens are never, always, or switchably available.
    pub thinking: ThinkingMode,
    /// Capability metadata advertised on the catalog.
    pub capabilities: Capabilities,
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

/// Build one shared [`DominionQueue`] per configured dominion.
///
/// Cloning a returned queue clones the Arc-backed limit, so everything bound
/// to the same dominion competes for one pool of slots. Remote endpoints
/// (routing) and local models ([`crate::local`]) both build their bindings
/// from this map; validation keeps the two on disjoint dominion kinds, so
/// each side only ever looks up its own kind's queues.
pub(crate) fn dominion_queues(config: &Config) -> HashMap<&str, DominionQueue> {
    let mut queues = HashMap::with_capacity(config.dominions().len());
    for dominion in config.dominions() {
        let queue = match dominion.max_concurrency() {
            Some(n) => DominionQueue::new(
                n,
                dominion.max_queue(),
                dominion.fair_scheduling(),
                dominion.policy(),
            ),
            // Unlimited concurrency never parks a caller, so `max_queue`
            // and `policy` have no wait to bound.
            None => DominionQueue::unlimited(),
        };
        queues.insert(dominion.id(), queue);
    }
    queues
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
            let tool_dialect = model.tool_dialect();
            models.push(Arc::new(Model {
                name: model.name().to_owned(),
                kind: model.kind(),
                description: model.description().to_owned(),
                context: model.context(),
                thinking: model.thinking(),
                capabilities: model.capabilities().clone(),
                tool_dialect: tool_dialect.to_string(),
                tools_mode: match tool_dialect {
                    ToolDialect::Gemma3ToolCode => "emulated",
                    _ => "native",
                }
                .to_owned(),
                upstream_name: model.upstream().to_owned(),
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

/// Guard that a resolved model serves the workload its route handles, so a
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
            queue: DominionQueue::unlimited(),
        });
        Arc::new(Model {
            name: name.to_owned(),
            kind: ModelKind::Chat,
            description: "d".to_owned(),
            context: 8192,
            thinking: ThinkingMode::Never,
            capabilities: Capabilities::default(),
            tool_dialect: "openai".to_owned(),
            tools_mode: "native".to_owned(),
            upstream_name: "u".to_owned(),
            endpoint,
        })
    }

    fn routing_from(toml: &str) -> Routing {
        let config = Config::from_toml_str(toml).unwrap();
        Routing::from_config(&config).unwrap()
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
        routing_from(toml)
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
    fn from_config_carries_model_kinds() {
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
name = "chatty"
description = "a chat model"
context = 8192
upstream = "u"
endpoints = ["e"]

[[model]]
name = "embed"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "u"
endpoints = ["e"]
"#;
        let routing = routing_from(toml);
        assert_eq!(routing.model("chatty").unwrap().kind, ModelKind::Chat);
        assert_eq!(routing.model("embed").unwrap().kind, ModelKind::Embedding);
    }

    #[test]
    fn from_config_carries_tool_dialect_and_derives_tools_mode() {
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
name = "plain"
description = "a native-tools model"
context = 8192
upstream = "u"
endpoints = ["e"]

[[model]]
name = "gemma"
description = "an emulated-tools model"
context = 8192
tool_dialect = "gemma3_tool_code"
upstream = "u"
endpoints = ["e"]
"#;
        let routing = routing_from(toml);
        let plain = routing.model("plain").unwrap();
        assert_eq!(plain.tool_dialect, "openai");
        assert_eq!(plain.tools_mode, "native");
        let gemma = routing.model("gemma").unwrap();
        assert_eq!(gemma.tool_dialect, "gemma3_tool_code");
        assert_eq!(gemma.tools_mode, "emulated");
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

    #[tokio::test]
    async fn endpoints_on_one_dominion_share_one_limit() {
        // The new behavior dominions introduce: two endpoints bound to one
        // dominion compete for a single pool of slots. Filling the queue
        // through one endpoint blocks the other.
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "pool"
kind = "remote"
max_concurrency = 1

[[endpoint]]
id = "a"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""
dominion = "pool"

[[endpoint]]
id = "b"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""
dominion = "pool"

[[model]]
name = "ma"
description = "model on endpoint a"
context = 8192
upstream = "ua"
endpoints = ["a"]

[[model]]
name = "mb"
description = "model on endpoint b"
context = 8192
upstream = "ub"
endpoints = ["b"]
"#;
        let routing = routing_from(toml);
        let queue_a = routing.model("ma").unwrap().endpoint.queue.clone();
        let queue_b = routing.model("mb").unwrap().endpoint.queue.clone();

        // Fill the dominion's only slot through endpoint A.
        let held = queue_a.admit("client").await.unwrap();

        // Endpoint B's admit cannot proceed: it parks as a waiter on the SAME
        // shared queue instead of getting a slot of its own.
        let queue_b_spawn = queue_b.clone();
        let blocked = tokio::spawn(async move { queue_b_spawn.admit("client").await });
        while queue_a.waiter_count() != 1 {
            tokio::task::yield_now().await;
        }

        // Releasing A's permit hands the shared slot to B's waiter.
        drop(held);
        let _permit = blocked.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn dominion_without_max_concurrency_is_unlimited() {
        // Absent max_concurrency means unlimited: admits never wait, so a
        // bound max_queue and reject policy have no full in-flight set to
        // act on.
        let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "pool"
kind = "remote"
max_queue = 1
policy = "reject"

[[endpoint]]
id = "a"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""
dominion = "pool"

[[model]]
name = "ma"
description = "model on endpoint a"
context = 8192
upstream = "ua"
endpoints = ["a"]
"#;
        let routing = routing_from(toml);
        let queue = routing.model("ma").unwrap().endpoint.queue.clone();
        let _first = queue.admit("client").await.unwrap();
        let _second = queue.admit("client").await.unwrap();
        let _third = queue.admit("client").await.unwrap();
    }
}
