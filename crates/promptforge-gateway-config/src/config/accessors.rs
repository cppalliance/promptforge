//! Read accessors for the validated configuration types.
//!
//! Every field is private; this module is the read API. Values are only
//! reachable through validating construction ([`Config::load`],
//! [`Config::load_profile`], [`Config::from_toml_str`]), so accessors never
//! re-check operator input.

use std::net::SocketAddr;

use super::{
    Config, DeviceConfig, DeviceKind, EndpointConfig, LaneConfig, LocalConfig, LocalModelConfig,
    ModelConfig, Protocol, SearchProvider, Secret, ServerConfig, ThinkingMode, ToolsConfig,
    WebSearchConfig,
};
use crate::queue::QueueConfig;

impl Config {
    /// Returns the `[server]` section: bind address and shared bearer key.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.server().bind().port(), 8080);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn server(&self) -> &ServerConfig {
        &self.server
    }

    /// Returns the `[queue]` waiting-queue settings.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [queue]
    /// # max_depth = 50
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.queue().max_depth(), 50);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn queue(&self) -> &QueueConfig {
        &self.queue
    }

    /// Returns the `[local]` section: cache settings for gateway-owned local
    /// inference.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [local]
    /// # cache_dir = "/tmp/pf-models"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local().cache_dir(), Some("/tmp/pf-models"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn local(&self) -> &LocalConfig {
        &self.local
    }

    /// Returns the configured `[[device]]` compute resources.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.devices()[0].id(), "gpu0");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn devices(&self) -> &[DeviceConfig] {
        &self.devices
    }

    /// Returns the configured `[[endpoint]]` backends.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].id(), "e");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn endpoints(&self) -> &[EndpointConfig] {
        &self.endpoints
    }

    /// Returns the `[[model]]` routing table from model name to remote
    /// backend.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// #
    /// # [[model]]
    /// # name = "m"
    /// # description = "a model"
    /// # context = 8192
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].name(), "m");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn models(&self) -> &[ModelConfig] {
        &self.models
    }

    /// Returns the `[[local_model]]` entries served by managed
    /// `llama-server` children.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].name(), "q");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn local_models(&self) -> &[LocalModelConfig] {
        &self.local_models
    }

    /// Returns the `[tools]` configuration, or `None` when the section is
    /// absent.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.tools().is_some_and(|tools| tools.web_search().is_some()));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn tools(&self) -> Option<&ToolsConfig> {
        self.tools.as_ref()
    }
}

impl ServerConfig {
    /// Returns the socket address the gateway binds.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.server().bind().to_string(), "127.0.0.1:8080");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// Returns the shared bearer key every `/v1/*` request must present.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.server().api_key().expose(), "secret");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }
}

impl LocalConfig {
    /// Returns the root directory for GGUF files and the pinned
    /// `llama-server` install, or `None` for the default `~/.promptforge`.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [local]
    /// # cache_dir = "/tmp/pf-models"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local().cache_dir(), Some("/tmp/pf-models"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn cache_dir(&self) -> Option<&str> {
        self.cache_dir.as_deref()
    }
}
impl DeviceConfig {
    /// Returns the operator-chosen device id referenced by endpoints and
    /// local models.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.devices()[0].id(), "gpu0");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns whether the device is a remote provider or a local GPU.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, DeviceKind};
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.devices()[0].kind(), DeviceKind::Local);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn kind(&self) -> DeviceKind {
        self.kind
    }

    /// Returns the max concurrent requests for a remote device, or `None`
    /// for a local device (which uses lanes).
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "runpod"
    /// # type = "remote"
    /// # concurrency = 4
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.devices()[0].concurrency(), Some(4));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn concurrency(&self) -> Option<usize> {
        self.concurrency
    }

    /// Returns the device's `[[device.lane]]` concurrency lanes.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// #
    /// # [[device.lane]]
    /// # id = "generative"
    /// # concurrency = 2
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.devices()[0].lanes()[0].id(), "generative");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn lanes(&self) -> &[LaneConfig] {
        &self.lanes
    }
}

impl LaneConfig {
    /// Returns the lane id referenced by `[[local_model]].lane`.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// #
    /// # [[device.lane]]
    /// # id = "generative"
    /// # concurrency = 2
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.devices()[0].lanes()[0].id(), "generative");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the max concurrent inferences on this lane.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// #
    /// # [[device.lane]]
    /// # id = "generative"
    /// # concurrency = 2
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.devices()[0].lanes()[0].concurrency(), 2);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// Returns the explicit device id, when the lane was declared outside its
    /// parent's `[[device]]` table.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// #
    /// # [[device.lane]]
    /// # id = "generative"
    /// # concurrency = 2
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.devices()[0].lanes()[0].device(), None);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }
}

impl EndpointConfig {
    /// Returns the endpoint's id: the operator-chosen handle referenced by
    /// `[[model]]` entries.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].id(), "e");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the wire protocol this endpoint speaks.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, Protocol};
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].protocol(), Protocol::Openai);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Returns the backend base URL.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].base_url(), "http://127.0.0.1:9");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the credential sent to this backend.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = "backend-key"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].api_key().expose(), "backend-key");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }

    /// Returns the maximum in-flight requests to this endpoint, or `None`
    /// for unlimited.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # concurrency = 4
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].concurrency(), Some(4));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn concurrency(&self) -> Option<usize> {
        self.concurrency
    }

    /// Returns the remote device id whose concurrency governs this endpoint,
    /// when set.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "runpod"
    /// # type = "remote"
    /// # concurrency = 4
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # device = "runpod"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].device(), Some("runpod"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }
}
impl ModelConfig {
    /// Returns the name callers request and that a slot resolves to.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// #
    /// # [[model]]
    /// # name = "m"
    /// # description = "a model"
    /// # context = 8192
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].name(), "m");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the prose describing the model for catalog consumers and
    /// semantic bind.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// #
    /// # [[model]]
    /// # name = "m"
    /// # description = "a model"
    /// # context = 8192
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].description(), "a model");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the context window size in tokens.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// #
    /// # [[model]]
    /// # name = "m"
    /// # description = "a model"
    /// # context = 8192
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].context(), 8192);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn context(&self) -> u32 {
        self.context
    }

    /// Returns whether thinking tokens are never, always, or switchably
    /// available.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, ThinkingMode};
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// #
    /// # [[model]]
    /// # name = "m"
    /// # description = "a model"
    /// # context = 8192
    /// # thinking = "switchable"
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].thinking(), ThinkingMode::Switchable);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }

    /// Returns the string the backend knows this model by.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// #
    /// # [[model]]
    /// # name = "m"
    /// # description = "a model"
    /// # context = 8192
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].upstream(), "u");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn upstream(&self) -> &str {
        &self.upstream
    }

    /// Returns the endpoint ids serving this model (v0 uses the first).
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// #
    /// # [[model]]
    /// # name = "m"
    /// # description = "a model"
    /// # context = 8192
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].endpoints(), ["e"]);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    /// Returns the `max_tokens` default supplied when the caller omits one.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// #
    /// # [[model]]
    /// # name = "m"
    /// # description = "a model"
    /// # context = 8192
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # default_max_tokens = 1024
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].default_max_tokens(), Some(1024));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn default_max_tokens(&self) -> Option<u32> {
        self.default_max_tokens
    }
}
impl LocalModelConfig {
    /// Returns the caller-facing model name in `/v1/models` and chat
    /// completions.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].name(), "q");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the prose describing the model for catalog consumers and
    /// semantic bind.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].description(), "a local model");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the model source: an https URL or a local filesystem path to
    /// a GGUF.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].source(), "/models/q.gguf");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the SHA-256 pin (lowercase hex) verified after download, when
    /// set.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let digest = "a".repeat(64);
    /// # let toml = format!(r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "https://example.com/q.gguf"
    /// # sha256 = "{digest}"
    /// # context = 4096
    /// # "#);
    /// let config = Config::from_toml_str(&toml)?;
    /// assert_eq!(config.local_models()[0].sha256(), Some(digest.as_str()));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    /// Returns the device id (`[[device]]`) binding this model, when set.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// #
    /// # [[device.lane]]
    /// # id = "generative"
    /// # concurrency = 2
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # device = "gpu0"
    /// # lane = "generative"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].device(), Some("gpu0"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn device(&self) -> Option<&str> {
        self.device.as_deref()
    }

    /// Returns the lane id under the device (`[[device.lane]]`), when set.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[device]]
    /// # id = "gpu0"
    /// # type = "local"
    /// #
    /// # [[device.lane]]
    /// # id = "generative"
    /// # concurrency = 2
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # device = "gpu0"
    /// # lane = "generative"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].lane(), Some("generative"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn lane(&self) -> Option<&str> {
        self.lane.as_deref()
    }

    /// Returns the context window size in tokens (`--ctx-size`).
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].context(), 4096);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn context(&self) -> u32 {
        self.context
    }

    /// Returns whether thinking tokens are never, always, or switchably
    /// available.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, ThinkingMode};
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # thinking = "always"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].thinking(), ThinkingMode::Always);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }

    /// Returns the GPU layers offloaded (`-ngl`).
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # gpu_layers = 40
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].gpu_layers(), 40);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn gpu_layers(&self) -> u32 {
        self.gpu_layers
    }

    /// Returns whether flash attention is enabled (`--flash-attn on`).
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # flash_attention = false
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(!config.local_models()[0].flash_attention());
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn flash_attention(&self) -> bool {
        self.flash_attention
    }

    /// Returns the KV cache type for K.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # cache_type_k = "f16"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].cache_type_k(), "f16");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn cache_type_k(&self) -> &str {
        &self.cache_type_k
    }

    /// Returns the KV cache type for V.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # cache_type_v = "f16"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].cache_type_v(), "f16");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn cache_type_v(&self) -> &str {
        &self.cache_type_v
    }

    /// Returns the generation ceiling (`--n-predict`).
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # n_predict = 256
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].n_predict(), 256);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn n_predict(&self) -> u32 {
        self.n_predict
    }

    /// Returns the path to a Jinja chat template file
    /// (`--chat-template-file`), when set.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # chat_template_file = "q.jinja"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].chat_template_file(), Some("q.jinja"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn chat_template_file(&self) -> Option<&str> {
        self.chat_template_file.as_deref()
    }
}
impl ToolsConfig {
    /// Returns the web-search tool configuration, or `None` when no
    /// `[tools.web_search]` section is present.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let tools = config.tools().expect("tools section present");
    /// assert!(tools.web_search().is_some());
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn web_search(&self) -> Option<&WebSearchConfig> {
        self.web_search.as_ref()
    }
}

impl WebSearchConfig {
    /// Returns the search provider backing the tool.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, SearchProvider};
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.provider(), SearchProvider::Brave);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn provider(&self) -> SearchProvider {
        self.provider
    }

    /// Returns the credential sent to the search provider.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.api_key().expose(), "k");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }

    /// Returns the search API base URL.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # base_url = "https://search.example.com/res/v1"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.base_url(), "https://search.example.com/res/v1");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the result count used when the request omits `count`.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # default_count = 5
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.default_count(), 5);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn default_count(&self) -> u8 {
        self.default_count
    }

    /// Returns the clamp and over-fetch ceiling for result counts.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # max_count = 15
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.max_count(), 15);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn max_count(&self) -> u8 {
        self.max_count
    }

    /// Returns the diversity cap per hostname group.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # max_per_host = 3
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.max_per_host(), 3);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn max_per_host(&self) -> u8 {
        self.max_per_host
    }

    /// Returns the freshness filter applied when the request omits
    /// `freshness` (empty means omit).
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # default_freshness = "pw"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.default_freshness(), "pw");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn default_freshness(&self) -> &str {
        &self.default_freshness
    }

    /// Returns the safesearch setting applied when the request omits
    /// `safesearch` (empty means omit).
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # default_safesearch = "moderate"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.default_safesearch(), "moderate");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn default_safesearch(&self) -> &str {
        &self.default_safesearch
    }

    /// Returns whether known tracking query params are scrubbed from result
    /// URLs.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # strip_tracking = false
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert!(!search.strip_tracking());
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn strip_tracking(&self) -> bool {
        self.strip_tracking
    }
}
