//! The agent sessions' search provider: every `web/search`
//! call runs through the current Gateway generation's web search relay.
//!
//! The provider holds the server's [`Registry`] and reads the gateway
//! handles through it on every search, as the inference broker does, so a
//! replaced gateway serves the next search. It keeps the [`GatewaySearch`]
//! of the last generation it searched under and hands each search to it.

use std::sync::{Mutex, PoisonError};

use harness_gateway_client::GatewaySearch;
use harness_web::{SearchError, SearchErrorKind, SearchProvider, SearchQuery, SearchResults};
use workshop_gateway::GatewayHandles;
use workshop_registry::Registry;

use super::gateway::UsableGateway;

/// The message a search fails with when no usable Gateway client exists,
/// so no endpoint or key detail reaches the model.
const NO_GATEWAY: &str = "request failed";

/// Searches through the Gateway the server's registry currently holds.
pub(super) struct GatewaySearchProvider {
    /// The subsystem registry the gateway handles are read through.
    registry: Registry,
    /// The client built for the last generation searched under.
    cached: Mutex<Option<(u64, GatewaySearch)>>,
}

impl GatewaySearchProvider {
    /// Builds the provider over the server's subsystem registry.
    pub(super) fn new(registry: Registry) -> Self {
        Self {
            registry,
            cached: Mutex::new(None),
        }
    }

    /// The client for the current gateway generation, built on the first
    /// search under it. A missing gateway registration, a gateway the
    /// heartbeat reports down, or an endpoint or key that cannot be built
    /// fails as transport with [`NO_GATEWAY`].
    fn client(&self) -> Result<GatewaySearch, SearchError> {
        let no_gateway = || SearchError::new(SearchErrorKind::Transport, NO_GATEWAY);
        let handles = self
            .registry
            .state::<GatewayHandles>()
            .ok_or_else(no_gateway)?;
        if !handles.health().is_reachable() {
            return Err(no_gateway());
        }
        let gateway = UsableGateway::read(&handles).ok_or_else(no_gateway)?;
        // A poisoned lock holds a pair written whole by one store.
        let mut cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((generation, client)) = cached.as_ref()
            && *generation == gateway.generation
        {
            return Ok(client.clone());
        }
        let client = GatewaySearch::new(gateway.endpoint, gateway.key);
        *cached = Some((gateway.generation, client.clone()));
        Ok(client)
    }
}

#[async_trait::async_trait]
impl SearchProvider for GatewaySearchProvider {
    async fn search(&self, query: SearchQuery) -> Result<SearchResults, SearchError> {
        let client = self.client()?;
        SearchProvider::search(&client, query).await
    }
}

#[cfg(test)]
#[path = "search-tests.rs"]
mod tests;
