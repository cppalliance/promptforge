//! The one read of a usable gateway that the agent sessions' broker,
//! search provider, and launch refusal share: the generation, the API
//! root, and the bearer of the gateway the registry currently holds.
//!
//! It reads no heartbeat health; a caller that needs reachability checks
//! it itself.

use harness_gateway_client::{GatewayEndpoint, SecretString};
use workshop_gateway::GatewayHandles;
use workshop_registry::Registry;

/// One gateway generation whose endpoint and key both build.
pub(super) struct UsableGateway {
    /// The generation the server assigned before publishing it.
    pub(super) generation: u64,
    /// The gateway's OpenAI-compatible API root: its base URL with `/v1`.
    pub(super) endpoint: GatewayEndpoint,
    /// The bearer paired with the base URL.
    pub(super) key: SecretString,
}

impl UsableGateway {
    /// The current generation of `handles`, or `None` when its URL or key
    /// cannot build an endpoint and a bearer.
    pub(super) fn read(handles: &GatewayHandles) -> Option<Self> {
        let snapshot = handles.binding().snapshot();
        let api_root = format!("{}/v1", snapshot.base_url().trim_end_matches('/'));
        Some(Self {
            generation: snapshot.generation(),
            endpoint: GatewayEndpoint::new(&api_root).ok()?,
            key: SecretString::new(snapshot.api_key()).ok()?,
        })
    }
}

/// The gateway `registry` holds, or `None` when no gateway handles are
/// registered or the current generation's URL or key cannot build.
pub(super) fn usable_gateway(registry: &Registry) -> Option<UsableGateway> {
    let handles = registry.state::<GatewayHandles>()?;
    UsableGateway::read(&handles)
}
