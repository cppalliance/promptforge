//! The agent sessions' search provider: every `promptforge/web/search`
//! call runs through the current Gateway generation's web search relay.
//!
//! The provider holds the server's [`Registry`] and reads the gateway
//! handles through it on every search, as `push_bindings` does, so a
//! replaced gateway serves the next search. It keeps the client of the
//! last generation it searched under. The paths into `harness_gateway_client`
//! stay qualified, because `workshop_gateway` has a `GatewayClient` too.

use std::sync::{Mutex, PoisonError};

use harness_web::{
    SearchError, SearchErrorKind, SearchProvider, SearchQuery, SearchResult, SearchResults,
};
use workshop_gateway::GatewayHandles;
use workshop_registry::Registry;

use super::bindings::gateway_binding;

/// The message a search fails with when no usable Gateway client exists,
/// so no endpoint or key detail reaches the model.
const NO_GATEWAY: &str = "request failed";

/// Searches through the Gateway the server's registry currently holds.
pub(crate) struct GatewaySearchProvider {
    /// The subsystem registry the gateway handles are read through.
    registry: Registry,
    /// The client built for the last generation searched under.
    cached: Mutex<Option<(u64, harness_gateway_client::GatewaySearch)>>,
}

impl GatewaySearchProvider {
    /// Builds the provider over the server's subsystem registry.
    pub(crate) fn new(registry: Registry) -> Self {
        Self {
            registry,
            cached: Mutex::new(None),
        }
    }

    /// The client for the current gateway generation, built on the first
    /// search under it. A missing gateway registration, a gateway the
    /// heartbeat reports down, or an endpoint or key that cannot be built
    /// fails as transport with [`NO_GATEWAY`].
    fn client(&self) -> Result<harness_gateway_client::GatewaySearch, SearchError> {
        let no_gateway = || SearchError::new(SearchErrorKind::Transport, NO_GATEWAY);
        let handles = self
            .registry
            .state::<GatewayHandles>()
            .ok_or_else(no_gateway)?;
        if !handles.health().is_reachable() {
            return Err(no_gateway());
        }
        let snapshot = handles.binding().snapshot();
        // A poisoned lock holds a pair written whole by one store.
        let mut cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((generation, client)) = cached.as_ref()
            && *generation == snapshot.generation()
        {
            return Ok(client.clone());
        }
        let binding = gateway_binding(&snapshot);
        let endpoint = harness_gateway_client::GatewayEndpoint::new(&binding.api_root())
            .map_err(|_| no_gateway())?;
        let key =
            harness_gateway_client::SecretString::new(binding.key).map_err(|_| no_gateway())?;
        let client = harness_gateway_client::GatewaySearch::new(endpoint, key);
        *cached = Some((binding.generation, client.clone()));
        Ok(client)
    }
}

#[async_trait::async_trait]
impl SearchProvider for GatewaySearchProvider {
    async fn search(&self, query: SearchQuery) -> Result<SearchResults, SearchError> {
        let client = self.client()?;
        let response = client
            .search(&gateway_request(query))
            .await
            .map_err(search_error)?;
        Ok(search_results(response))
    }
}

/// The Gateway's request for a validated query.
fn gateway_request(query: SearchQuery) -> harness_gateway_client::GatewaySearchRequest {
    harness_gateway_client::GatewaySearchRequest {
        query: query.query,
        count: query.count,
        freshness: query
            .freshness
            .map(|freshness| freshness.as_str().to_owned()),
        country: query.country,
        search_lang: query.search_lang,
        safesearch: query.safesearch.map(|level| level.as_str().to_owned()),
        include_domains: query.include_domains,
        exclude_domains: query.exclude_domains,
    }
}

/// The provider's results for the Gateway's reply.
fn search_results(response: harness_gateway_client::GatewaySearchResponse) -> SearchResults {
    SearchResults {
        query: response.query,
        results: response
            .results
            .into_iter()
            .map(|result| SearchResult {
                title: result.title,
                url: result.url,
                description: result.description,
                age: result.age,
                site_name: result.site_name,
                extra_snippets: result.extra_snippets,
            })
            .collect(),
    }
}

/// The provider's error for a failed Gateway search: its kind and text,
/// with the Gateway's error as the cause.
fn search_error(error: harness_gateway_client::GatewaySearchError) -> SearchError {
    let kind = if error.kind() == harness_gateway_client::GatewaySearchErrorKind::Backend {
        SearchErrorKind::Backend
    } else {
        SearchErrorKind::Transport
    };
    SearchError::with_source(kind, error.to_string(), error)
}

#[cfg(test)]
#[path = "search-tests.rs"]
mod tests;
