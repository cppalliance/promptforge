//! Read accessors for the validated configuration types.
//!
//! Every field is private; this module is the read API. Values are only
//! reachable through validating construction ([`Config::load`] or
//! [`Config::from_toml_str`]), so accessors never re-check operator input.

use std::net::SocketAddr;

use super::{
    Capabilities, Config, DominionConfig, DominionKind, EndpointConfig, LlamaBackend, LocalConfig,
    LocalModelConfig, ModelConfig, ModelKind, ProfileConfig, Protocol, QueuePolicy, SearchProvider,
    Secret, ServerConfig, SttModelConfig, ThinkingMode, ToolDialect, ToolsConfig, WebSearchConfig,
    WorkshopConfig,
};

impl Config {
    /// Returns the on-disk schema version.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n",
    /// )?;
    /// assert_eq!(config.config_version(), 2);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn config_version(&self) -> u32 {
        self.version
    }

    /// Returns the `[server]` section: bind address and shared bearer key.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.server().bind().port(), 8080);
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [local]
    /// # cache_dir = "/tmp/pf-models"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local().cache_dir(), Some("/tmp/pf-models"));
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn local(&self) -> &LocalConfig {
        &self.local
    }

    /// Returns the configured `[[dominion]]` compute pools.
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
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # max_concurrency = 4
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].id(), "runpod-pool");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn dominions(&self) -> &[DominionConfig] {
        &self.dominions
    }

    /// Returns the configured `[[endpoint]]` backends.
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
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].id(), "e");
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn local_models(&self) -> &[LocalModelConfig] {
        &self.local_models
    }

    /// Returns the active profile's speech-to-text models.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::{Config, ProfileName};
    /// let catalog = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    ///      [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n\
    ///      [[profile]]\nname = \"work\"\nmodels = [\"speech\"]\n",
    /// )?;
    /// let config = catalog.select_profile(&ProfileName::parse("work")?)?;
    /// assert_eq!(config.stt_models()[0].name(), "speech");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn stt_models(&self) -> &[SttModelConfig] {
        &self.stt_models
    }

    /// Returns every remote model in the global catalog.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n",
    /// )?;
    /// assert!(config.catalog_models().is_empty());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn catalog_models(&self) -> &[ModelConfig] {
        &self.catalog_models
    }

    /// Returns every local chat model in the global catalog.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n",
    /// )?;
    /// assert!(config.catalog_local_models().is_empty());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn catalog_local_models(&self) -> &[LocalModelConfig] {
        &self.catalog_local_models
    }

    /// Returns every speech-to-text model in the global catalog.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n",
    /// )?;
    /// assert!(config.catalog_stt_models().is_empty());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn catalog_stt_models(&self) -> &[SttModelConfig] {
        &self.catalog_stt_models
    }

    /// Returns every profile checklist in declaration order.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    ///      [[profile]]\nname = \"work\"\nmodels = []\n",
    /// )?;
    /// assert_eq!(config.profiles()[0].name(), "work");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn profiles(&self) -> &[ProfileConfig] {
        &self.profiles
    }

    /// Returns the active profile, or `None` for an unselected in-memory
    /// document.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::{Config, ProfileName};
    /// let catalog = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    ///      [[profile]]\nname = \"work\"\nmodels = []\n",
    /// )?;
    /// assert!(catalog.active_profile().is_none());
    /// let config = catalog.select_profile(&ProfileName::parse("work")?)?;
    /// assert_eq!(
    ///     config.active_profile().map(|profile| profile.name()),
    ///     Some("work")
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn active_profile(&self) -> Option<&ProfileConfig> {
        self.active_profile.map(|index| &self.profiles[index])
    }

    /// Returns the `[tools]` configuration, or `None` when the section is
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
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.tools().is_some_and(|tools| tools.web_search().is_some()));
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn tools(&self) -> Option<&ToolsConfig> {
        self.tools.as_ref()
    }

    /// Returns the `[workshop]` configuration, or `None` when the section is
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
    /// # [workshop]
    /// # bind = "127.0.0.1:7910"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.workshop().is_some());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn workshop(&self) -> Option<&WorkshopConfig> {
        self.workshop.as_ref()
    }
}

impl ProfileConfig {
    /// Returns the profile identifier.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    ///      [[profile]]\nname = \"work\"\nmodels = []\n",
    /// )?;
    /// assert_eq!(config.profiles()[0].name(), "work");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the selected global catalog names in declaration order.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// let config = Config::from_toml_str(
    ///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    ///      [[profile]]\nname = \"work\"\nmodels = []\n",
    /// )?;
    /// assert!(config.profiles()[0].models().is_empty());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn models(&self) -> &[String] {
        &self.models
    }
}

impl ServerConfig {
    /// Returns the socket address the gateway binds.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.server().bind().to_string(), "127.0.0.1:8080");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// Returns the shared bearer key every `/v1/*` request must present.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.server().api_key().expose(), "secret");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }

    /// Returns whether a loopback peer presenting no credential is admitted
    /// to every route. On by default; an operator on a shared machine sets
    /// `trust_loopback = false` to require the bearer key from every caller.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// # trust_loopback = false
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(!config.server().trust_loopback());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn trust_loopback(&self) -> bool {
        self.trust_loopback
    }

    /// Returns the base URL a same-host client uses to reach this server,
    /// loopback-adjusted: an unspecified bind IP (`0.0.0.0` or `::`) is not
    /// a reachable destination, so it becomes the matching loopback address;
    /// every other address is kept verbatim.
    ///
    /// This is how the hosted workshop derives its gateway `base_url` from
    /// `[server]` at boot (paired with the same `api_key`), so no credential
    /// or address is duplicated in `[workshop]`.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "0.0.0.0:8081"
    /// # api_key = "secret"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.server().client_url(), "http://127.0.0.1:8081");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn client_url(&self) -> String {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

        let mut addr = self.bind;
        match addr.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => {
                addr.set_ip(IpAddr::V4(Ipv4Addr::LOCALHOST));
            }
            IpAddr::V6(ip) if ip.is_unspecified() => {
                addr.set_ip(IpAddr::V6(Ipv6Addr::LOCALHOST));
            }
            _ => {}
        }
        format!("http://{addr}")
    }
}

impl LocalConfig {
    /// Returns the root directory for GGUF files and the pinned
    /// `llama-server` install, or `None` for the default `~/.promptforge`.
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
    /// # [local]
    /// # cache_dir = "/tmp/pf-models"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local().cache_dir(), Some("/tmp/pf-models"));
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn cache_dir(&self) -> Option<&str> {
        self.cache_dir.as_deref()
    }

    /// Returns the configured `llama-server` backend selection
    /// (`llama_backend`, default `auto`). Consulted only on Windows x86-64.
    #[must_use]
    pub fn llama_backend(&self) -> LlamaBackend {
        self.llama_backend
    }

    /// Returns the explicit `llama-server` executable path
    /// (`llama_server_path`), when set.
    #[must_use]
    pub fn llama_server_path(&self) -> Option<&str> {
        self.llama_server_path.as_deref()
    }
}
impl DominionConfig {
    /// Returns the operator-chosen dominion id referenced by endpoints and
    /// local models.
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
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].id(), "runpod-pool");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns whether the dominion pools remote providers or local GPUs.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::{Config, DominionKind};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn max_concurrency(&self) -> Option<usize> {
        self.max_concurrency
    }

    /// Returns the max waiting requests before new admits are rejected.
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
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # max_queue = 50
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.dominions()[0].max_queue(), 50);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn max_queue(&self) -> usize {
        self.max_queue
    }

    /// Returns whether a full queue waits or rejects.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::{Config, QueuePolicy};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn policy(&self) -> QueuePolicy {
        self.policy
    }

    /// Returns whether waiting callers are served round-robin by client key.
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
    /// # [[dominion]]
    /// # id = "runpod-pool"
    /// # kind = "remote"
    /// # fair_scheduling = false
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(!config.dominions()[0].fair_scheduling());
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the wire protocol this endpoint speaks.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::{Config, Protocol};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Returns the backend base URL.
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
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = ""
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].base_url(), "http://127.0.0.1:9");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the credential sent to this backend.
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
    /// # [[endpoint]]
    /// # id = "e"
    /// # protocol = "openai"
    /// # base_url = "http://127.0.0.1:9"
    /// # api_key = "backend-key"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.endpoints()[0].api_key().expose(), "backend-key");
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::{Config, ModelKind};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the context window size in tokens.
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::{Config, ThinkingMode};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }

    /// Returns the string the backend knows this model by.
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn upstream(&self) -> &str {
        &self.upstream
    }

    /// Returns the endpoint ids serving this model (v0 uses the first).
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    /// Returns the `max_tokens` default supplied when the caller omits one.
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::{Config, ToolDialect};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn tool_dialect(&self) -> ToolDialect {
        self.tool_dialect
    }

    /// Returns the capability metadata advertised on the catalog.
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::{Config, ModelKind};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let digest = "a".repeat(64);
    /// # let toml = format!(r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// assert_eq!(config.local_models()[0].vram_gb(), Some(14.0));
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn vram_gb(&self) -> Option<f64> {
        self.vram_gb
    }

    /// Returns the context window size in tokens (`--ctx-size`).
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
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].context(), 4096);
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::{Config, ThinkingMode};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }

    /// Returns the GPU layers offloaded (`-ngl`).
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
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # gpu_layers = 40
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].gpu_layers(), 40);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn gpu_layers(&self) -> u32 {
        self.gpu_layers
    }

    /// Returns whether flash attention is enabled (`--flash-attn on`).
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
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # flash_attention = false
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(!config.local_models()[0].flash_attention());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn flash_attention(&self) -> bool {
        self.flash_attention
    }

    /// Returns the KV cache type for K.
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
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # cache_type_k = "f16"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].cache_type_k(), "f16");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn cache_type_k(&self) -> &str {
        &self.cache_type_k
    }

    /// Returns the KV cache type for V.
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
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # cache_type_v = "f16"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].cache_type_v(), "f16");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn cache_type_v(&self) -> &str {
        &self.cache_type_v
    }

    /// Returns the generation ceiling (`--n-predict`).
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
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # n_predict = 256
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert_eq!(config.local_models()[0].n_predict(), 256);
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn chat_template_file(&self) -> Option<&str> {
        self.chat_template_file.as_deref()
    }

    /// Returns the capability metadata advertised on the catalog.
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn default_temperature(&self) -> Option<f32> {
        self.default_temperature
    }

    /// Returns whether the model accepts image inputs.
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn images(&self) -> bool {
        self.images
    }

    /// Returns whether the model can emit parallel tool calls.
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn effort_levels(&self) -> &[String] {
        &self.effort_levels
    }

    /// Returns the effort level applied when the caller omits one, when set.
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::{Config, SearchProvider};
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn provider(&self) -> SearchProvider {
        self.provider
    }

    /// Returns the credential sent to the search provider.
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
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.api_key().expose(), "k");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }

    /// Returns the search API base URL.
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
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # base_url = "https://search.example.com/res/v1"
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.base_url(), "https://search.example.com/res/v1");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the result count used when the request omits `count`.
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
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # default_count = 5
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.default_count(), 5);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn default_count(&self) -> u8 {
        self.default_count
    }

    /// Returns the clamp and over-fetch ceiling for result counts.
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
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # max_count = 15
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.max_count(), 15);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn max_count(&self) -> u8 {
        self.max_count
    }

    /// Returns the diversity cap per hostname group.
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
    /// # [tools.web_search]
    /// # provider = "brave"
    /// # api_key = "k"
    /// # max_per_host = 3
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// let search = config.web_search_config().expect("web_search present");
    /// assert_eq!(search.max_per_host(), 3);
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
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
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
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
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn strip_tracking(&self) -> bool {
        self.strip_tracking
    }
}
