//! Read accessors for the validated configuration types.
//!
//! Every field is private; this module is the read API. Values are only
//! reachable through validating construction ([`Config::load`] or
//! [`Config::from_toml_str`]), so accessors never re-check operator input.

use std::net::SocketAddr;

use super::{
    Config, DominionConfig, DominionKind, EndpointConfig, LlamaBackend, LocalConfig,
    LocalModelConfig, ModelConfig, ProfileConfig, Protocol, QueuePolicy, SearchProvider, Secret,
    ServerConfig, SttModelConfig, SttPipelineConfig, ToolsConfig, WebSearchConfig, WorkshopConfig,
};

#[path = "accessors-models.rs"]
mod models;

impl Config {
    /// Returns the on-disk schema version.
    #[must_use]
    pub fn config_version(&self) -> u32 {
        self.version
    }

    /// Returns the `[server]` section: bind address and shared bearer key.
    #[must_use]
    pub fn server(&self) -> &ServerConfig {
        &self.server
    }

    /// Returns the `[local]` section: cache settings for gateway-owned local
    /// inference.
    #[must_use]
    pub fn local(&self) -> &LocalConfig {
        &self.local
    }

    /// Returns the configured `[[dominion]]` compute pools.
    #[must_use]
    pub fn dominions(&self) -> &[DominionConfig] {
        &self.dominions
    }

    /// Returns the configured `[[endpoint]]` backends.
    #[must_use]
    pub fn endpoints(&self) -> &[EndpointConfig] {
        &self.endpoints
    }

    /// Returns the `[[model]]` routing table from model name to remote
    /// backend.
    ///
    /// Every remote model in the file is served regardless of the selected
    /// profile, so this is the whole remote catalog under any selection.
    #[must_use]
    pub fn models(&self) -> &[ModelConfig] {
        &self.models
    }

    /// Returns the active profile's `[[local_model]]` entries, the ones
    /// served by managed `llama-server` children.
    ///
    /// Empty when no profile is selected. An unselected in-memory document
    /// from [`Config::from_toml_str`] reports the whole local catalog.
    #[must_use]
    pub fn local_models(&self) -> &[LocalModelConfig] {
        &self.local_models
    }

    /// Returns the active profile's speech-to-text models.
    ///
    /// Empty when no profile is selected. An unselected in-memory document
    /// from [`Config::from_toml_str`] reports the whole speech-to-text
    /// catalog.
    #[must_use]
    pub fn stt_models(&self) -> &[SttModelConfig] {
        &self.stt_models
    }

    /// Returns every local chat model in the global catalog, whether or not
    /// the selected profile lists it.
    #[must_use]
    pub fn catalog_local_models(&self) -> &[LocalModelConfig] {
        &self.catalog_local_models
    }

    /// Returns every speech-to-text model in the global catalog, whether or
    /// not the selected profile lists it.
    #[must_use]
    pub fn catalog_stt_models(&self) -> &[SttModelConfig] {
        &self.catalog_stt_models
    }

    /// Returns every profile checklist in declaration order.
    #[must_use]
    pub fn profiles(&self) -> &[ProfileConfig] {
        &self.profiles
    }

    /// Returns the active profile, or `None` when no profile is selected.
    #[must_use]
    pub fn active_profile(&self) -> Option<&ProfileConfig> {
        self.active_profile.map(|index| &self.profiles[index])
    }

    /// Returns the profile name the sibling state file selected when that
    /// name is not defined in the configuration.
    ///
    /// [`Config::load`] degrades such a load to no profile instead of
    /// failing, so a boot can warn and a UI can show the stale selection.
    /// `None` for every other selection and for an in-memory document.
    #[must_use]
    pub fn stale_state_selection(&self) -> Option<&str> {
        self.stale_state_selection.as_deref()
    }

    /// Returns the `[tools]` configuration, or `None` when the section is
    /// absent.
    #[must_use]
    pub fn tools(&self) -> Option<&ToolsConfig> {
        self.tools.as_ref()
    }

    /// Returns canonical `[stt]` pipeline tuning, or `None` when absent.
    #[must_use]
    pub fn stt(&self) -> Option<&SttPipelineConfig> {
        self.stt.as_ref()
    }

    /// Returns the `[workshop]` configuration, or `None` when the section is
    /// absent.
    #[must_use]
    pub fn workshop(&self) -> Option<&WorkshopConfig> {
        self.workshop.as_ref()
    }
}

impl ProfileConfig {
    /// Returns the profile identifier.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the selected local and speech-to-text catalog names in
    /// declaration order.
    #[must_use]
    pub fn models(&self) -> &[String] {
        &self.models
    }
}

impl ServerConfig {
    /// Returns the socket address the gateway binds.
    #[must_use]
    pub fn bind(&self) -> SocketAddr {
        self.bind
    }

    /// Returns the shared bearer key every `/v1/*` request must present.
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }

    /// Returns whether a loopback peer presenting no credential is admitted
    /// to every route. On by default; an operator on a shared machine sets
    /// `trust_loopback = false` to require the bearer key from every caller.
    #[must_use]
    pub fn trust_loopback(&self) -> bool {
        self.trust_loopback
    }

    /// Returns the base URL a client on the same machine uses to reach this
    /// server, loopback-adjusted: an unspecified bind IP (`0.0.0.0` or `::`)
    /// is not a reachable destination, so it becomes the matching loopback
    /// address; every other address is kept verbatim.
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
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns whether the dominion pools remote providers or local GPUs.
    #[must_use]
    pub fn kind(&self) -> DominionKind {
        self.kind
    }

    /// Returns the max concurrent requests admitted across every binder, or
    /// `None` for unlimited.
    #[must_use]
    pub fn max_concurrency(&self) -> Option<usize> {
        self.max_concurrency
    }

    /// Returns the max waiting requests before new admits are rejected.
    #[must_use]
    pub fn max_queue(&self) -> usize {
        self.max_queue
    }

    /// Returns whether a full queue waits or rejects.
    #[must_use]
    pub fn policy(&self) -> QueuePolicy {
        self.policy
    }

    /// Returns whether waiting callers are served round-robin by client key.
    #[must_use]
    pub fn fair_scheduling(&self) -> bool {
        self.fair_scheduling
    }

    /// Returns the VRAM budget in gibibytes for co-residency checks, when
    /// set. Local kind only.
    #[must_use]
    pub fn vram_gb(&self) -> Option<u32> {
        self.vram_gb
    }
}

impl EndpointConfig {
    /// Returns the endpoint's id: the operator-chosen handle referenced by
    /// `[[model]]` entries.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the wire protocol this endpoint speaks.
    #[must_use]
    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Returns the backend base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the credential sent to this backend.
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }

    /// Returns the remote dominion id (`[[dominion]]`) whose shared limit
    /// and queue govern this endpoint, when set. Absent means unlimited
    /// pass-through.
    #[must_use]
    pub fn dominion(&self) -> Option<&str> {
        self.dominion.as_deref()
    }
}

impl ToolsConfig {
    /// Returns the web-search tool configuration, or `None` when no
    /// `[tools.web_search]` section is present.
    #[must_use]
    pub fn web_search(&self) -> Option<&WebSearchConfig> {
        self.web_search.as_ref()
    }
}

impl WebSearchConfig {
    /// Returns the search provider backing the tool.
    #[must_use]
    pub fn provider(&self) -> SearchProvider {
        self.provider
    }

    /// Returns the credential sent to the search provider.
    #[must_use]
    pub fn api_key(&self) -> &Secret {
        &self.api_key
    }

    /// Returns the search API base URL.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the result count used when the request omits `count`.
    #[must_use]
    pub fn default_count(&self) -> u8 {
        self.default_count
    }

    /// Returns the clamp and over-fetch ceiling for result counts.
    #[must_use]
    pub fn max_count(&self) -> u8 {
        self.max_count
    }

    /// Returns the diversity cap per hostname group.
    #[must_use]
    pub fn max_per_host(&self) -> u8 {
        self.max_per_host
    }

    /// Returns the freshness filter applied when the request omits
    /// `freshness` (empty means omit).
    #[must_use]
    pub fn default_freshness(&self) -> &str {
        &self.default_freshness
    }

    /// Returns the safesearch setting applied when the request omits
    /// `safesearch` (empty means omit).
    #[must_use]
    pub fn default_safesearch(&self) -> &str {
        &self.default_safesearch
    }

    /// Returns whether known tracking query params are scrubbed from result
    /// URLs.
    #[must_use]
    pub fn strip_tracking(&self) -> bool {
        self.strip_tracking
    }
}
