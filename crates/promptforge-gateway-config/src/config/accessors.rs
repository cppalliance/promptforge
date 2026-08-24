//! Read accessors for the validated configuration types.
//!
//! Every field is private; this module is the read API. Values are only
//! reachable through validating construction ([`Config::load`],
//! [`Config::load_profile`], [`Config::from_toml_str`]), so accessors never
//! re-check operator input.

use std::net::SocketAddr;

use super::{
    Capabilities, Config, DominionConfig, DominionKind, EndpointConfig, LocalConfig,
    LocalModelConfig, ModelConfig, ModelKind, Protocol, QueuePolicy, SearchProvider, Secret,
    ServerConfig, ThinkingMode, ToolDialect, ToolsConfig, WebSearchConfig,
};

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

    /// Returns the configured `[[dominion]]` compute pools.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # max_concurrency = 4
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].id(), "runpod-pool");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn dominions(&self) -> &[DominionConfig] {
        &self.dominions
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

    /// Returns the profile's `models` allowlist - the catalog subset the
    /// profile selected - or `None` when the merged document declares no
    /// allowlist and the full catalog is loaded.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # models = ["m"]
    /// #
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
    /// assert_eq!(config.model_allowlist(), Some(&["m".to_string()][..]));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn model_allowlist(&self) -> Option<&[String]> {
        self.model_allowlist.as_deref()
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
impl DominionConfig {
    /// Returns the operator-chosen dominion id referenced by endpoints and
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
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].id(), "runpod-pool");
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns whether the dominion pools remote providers or local GPUs.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, DominionKind};
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[dominion]]
    /// # id = "gpu0"
    /// # kind = "local"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].kind(), DominionKind::Local);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn kind(&self) -> DominionKind {
        self.kind
    }

    /// Returns the max concurrent requests admitted across every binder, or
    /// `None` for unlimited.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # max_concurrency = 4
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].max_concurrency(), Some(4));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn max_concurrency(&self) -> Option<usize> {
        self.max_concurrency
    }

    /// Returns the max waiting requests before new admits are rejected.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # max_queue = 50
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].max_queue(), 50);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn max_queue(&self) -> usize {
        self.max_queue
    }

    /// Returns whether a full queue waits or rejects.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, QueuePolicy};
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # policy = "reject"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].policy(), QueuePolicy::Reject);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn policy(&self) -> QueuePolicy {
        self.policy
    }

    /// Returns whether waiting callers are served round-robin by client key.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # fair_scheduling = false
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(!config.dominions()[0].fair_scheduling());
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn fair_scheduling(&self) -> bool {
        self.fair_scheduling
    }

    /// Returns the VRAM budget in gibibytes for co-residency checks, when
    /// set. Local kind only.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[dominion]]
    /// # id = "gpu0"
    /// # kind = "local"
    /// # vram_gb = 24
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].vram_gb(), Some(24));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn vram_gb(&self) -> Option<u32> {
        self.vram_gb
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

    /// Returns the remote dominion id (`[[dominion]]`) whose shared limit
    /// and queue govern this endpoint, when set. Absent means unlimited
    /// pass-through.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::Config;
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// #
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # dominion = "runpod-pool"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].dominion(), Some("runpod-pool"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn dominion(&self) -> Option<&str> {
        self.dominion.as_deref()
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

    /// Returns the workload this model serves: chat (the default),
    /// embedding, or classifier.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, ModelKind};
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
    /// # kind = "embedding"
    /// # description = "a model"
    /// # context = 8192
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].kind(), ModelKind::Embedding);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn kind(&self) -> ModelKind {
        self.kind
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

    /// Returns the tool-calling dialect this model speaks: `openai` (the
    /// default) for native wire tool calls, or `gemma3_tool_code` for
    /// emulated content-fence tool calling.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, ToolDialect};
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
    /// # tool_dialect = "gemma3_tool_code"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].tool_dialect(), ToolDialect::Gemma3ToolCode);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn tool_dialect(&self) -> ToolDialect {
        self.tool_dialect
    }

    /// Returns the capability metadata advertised on the catalog.
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
    /// # thinking = "switchable"
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # max_output = 4096
    /// # effort_levels = ["low", "high"]
    /// # default_effort = "low"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let capabilities = config.models()[0].capabilities();
    /// assert_eq!(capabilities.max_output(), Some(4096));
    /// assert_eq!(capabilities.effort_levels(), ["low", "high"]);
    /// assert_eq!(capabilities.default_effort(), Some("low"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
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

    /// Returns the workload this model serves: chat (the default),
    /// embedding, or classifier.
    ///
    /// # Examples
    /// ```
    /// # use promptforge_gateway_config::{Config, ModelKind};
    /// # let toml = r#"
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # kind = "classifier"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].kind(), ModelKind::Classifier);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn kind(&self) -> ModelKind {
        self.kind
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

    /// Returns the local dominion id (`[[dominion]]`) binding this model,
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
    /// # [[dominion]]
    /// # id = "gpu0"
    /// # kind = "local"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # dominion = "gpu0"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].dominion(), Some("gpu0"));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn dominion(&self) -> Option<&str> {
        self.dominion.as_deref()
    }

    /// Returns the max concurrent inferences: the child's `--parallel` value
    /// and, when no dominion is bound, the model's gateway queue limit.
    /// Defaults to 1.
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
    /// # parallel = 4
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].parallel(), 4);
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn parallel(&self) -> u32 {
        self.parallel
    }

    /// Returns the VRAM footprint estimate in gibibytes for the dominion
    /// co-residency check, when set.
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
    /// # vram_gb = 14
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].vram_gb(), Some(14));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn vram_gb(&self) -> Option<u32> {
        self.vram_gb
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

    /// Returns the capability metadata advertised on the catalog.
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
    /// # images = true
    /// # parallel_tool_calls = true
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let capabilities = config.local_models()[0].capabilities();
    /// assert!(capabilities.images());
    /// assert!(capabilities.parallel_tool_calls());
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
}

impl Capabilities {
    /// Returns the max output tokens the model can emit per completion, when
    /// set.
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
    /// # max_output = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.models()[0].capabilities().max_output(), Some(4096));
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn max_output(&self) -> Option<u32> {
        self.max_output
    }

    /// Returns the sampling temperature applied when the caller omits one,
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
    /// # default_temperature = 0.7
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(
    ///     config.models()[0].capabilities().default_temperature(),
    ///     Some(0.7)
    /// );
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn default_temperature(&self) -> Option<f32> {
        self.default_temperature
    }

    /// Returns whether the model accepts image inputs.
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
    /// # images = true
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.models()[0].capabilities().images());
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn images(&self) -> bool {
        self.images
    }

    /// Returns whether the model can emit parallel tool calls.
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
    /// # parallel_tool_calls = true
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.models()[0].capabilities().parallel_tool_calls());
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn parallel_tool_calls(&self) -> bool {
        self.parallel_tool_calls
    }

    /// Returns the reasoning-effort levels the model accepts (empty when the
    /// model has no effort knob).
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
    /// # thinking = "switchable"
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # effort_levels = ["low", "high"]
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(
    ///     config.models()[0].capabilities().effort_levels(),
    ///     ["low", "high"]
    /// );
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn effort_levels(&self) -> &[String] {
        &self.effort_levels
    }

    /// Returns the effort level applied when the caller omits one, when set.
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
    /// # thinking = "switchable"
    /// # upstream = "u"
    /// # endpoints = ["e"]
    /// # effort_levels = ["low", "high"]
    /// # default_effort = "low"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(
    ///     config.models()[0].capabilities().default_effort(),
    ///     Some("low")
    /// );
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn default_effort(&self) -> Option<&str> {
        self.default_effort.as_deref()
    }

    /// Returns whether the model adaptively chooses how much to think per
    /// request.
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
    /// # adaptive_thinking = true
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.models()[0].capabilities().adaptive_thinking());
    /// # Ok::<(), promptforge_gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn adaptive_thinking(&self) -> bool {
        self.adaptive_thinking
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
