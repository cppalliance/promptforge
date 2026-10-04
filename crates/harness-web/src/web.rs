//! [`Web`], the `promptforge/web` capability, and the keys of the two
//! services it reads.
//!
//! The fetch client is built once, at construction, with the fetch policy;
//! activation binds it to the run's runtime handle and builds the search
//! tool over the run's search provider. The prompt never sees either.

use std::sync::Arc;

use harness::capability::{
    Capability, CapabilityError, CapabilityErrorKind, CapabilityId, Contribution, RunServices,
    ServiceId, ServiceKey,
};
use tokio::runtime::Handle;

use crate::config::{ConfigError, FetchConfig};
use crate::fetch::FetchClient;
use crate::provider::SearchProvider;
use crate::search::WebSearch;

/// The Host's search provider, which every `promptforge/web/search` call
/// runs through.
pub const SEARCH_PROVIDER: ServiceKey<dyn SearchProvider> =
    ServiceKey::new("promptforge/search-provider");

/// The Host's tokio runtime, which every `promptforge/web/fetch` call is
/// spawned onto.
pub const TOKIO_RUNTIME: ServiceKey<Handle> = ServiceKey::new("promptforge/tokio-runtime");

/// The `promptforge/web` capability.
///
/// Contributes `promptforge/web/fetch` (a hardened page fetch rendering to
/// markdown) and `promptforge/web/search` (a search through the Host's
/// [`SearchProvider`]). It needs both [`SEARCH_PROVIDER`] and
/// [`TOKIO_RUNTIME`]: a run that requires the capability is refused when
/// either is missing, and a run that declares it optional gets no web
/// tools.
#[derive(Debug, Clone)]
pub struct Web {
    /// The stable identity, `promptforge/web`.
    id: CapabilityId,
    /// The fetch client, built over its validated policy.
    fetch: FetchClient,
}

impl Web {
    /// Builds the capability over the default fetch policy. The fetch
    /// client is built here, once, and every run's fetch tool shares it.
    ///
    /// # Panics
    /// Panics only if the literal capability id `promptforge/web` fails to
    /// parse, or the HTTP client cannot be built for the default policy
    /// (the TLS backend failed to initialize): defects, not caller errors.
    #[must_use]
    pub fn new() -> Web {
        #[expect(
            clippy::expect_used,
            reason = "the id is a literal of the capability id grammar; a parse failure is a defect in this file, not a caller-actionable condition"
        )]
        let id =
            CapabilityId::parse("promptforge/web").expect("the literal web capability id parses");
        Web {
            id,
            fetch: FetchClient::new(),
        }
    }

    /// Replaces the default fetch policy with a validated custom one.
    ///
    /// # Errors
    /// Returns [`ConfigError`] if the HTTP client cannot be built for
    /// `config` (for example a TLS backend that fails to initialize).
    pub fn with_fetch_config(mut self, config: FetchConfig) -> Result<Web, ConfigError> {
        self.fetch = FetchClient::try_with_config(config)?;
        Ok(self)
    }
}

impl Default for Web {
    fn default() -> Web {
        Web::new()
    }
}

impl Capability for Web {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Fetch a web page as markdown and search the web through the Host's search provider."
    }

    fn needs(&self) -> &[ServiceId] {
        const NEEDS: &[ServiceId] = &[SEARCH_PROVIDER.id(), TOKIO_RUNTIME.id()];
        NEEDS
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        if services.cancel.is_cancelled() {
            return Err(
                CapabilityError::message("promptforge/web: the run was cancelled")
                    .with_kind(CapabilityErrorKind::Cancelled),
            );
        }
        let (Some(provider), Some(runtime)) =
            (services.get(&SEARCH_PROVIDER), services.get(&TOKIO_RUNTIME))
        else {
            return Ok(Contribution::default());
        };
        Ok(Contribution {
            tools: vec![
                Arc::new(self.fetch.tool(Handle::clone(&runtime))),
                Arc::new(WebSearch::new(provider)),
            ],
            prelude: None,
        })
    }
}

#[cfg(test)]
#[path = "web-tests.rs"]
mod tests;
