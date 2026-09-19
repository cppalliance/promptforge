//! Gateway client acquisition.

use crate::client::GatewayClient;
use crate::{Error, Result};

use super::config::RunLimits;

/// Builds a gateway client from the environment with the run's HTTP limits
/// applied, so a lazily created client honors the same timeout and body cap as
/// a caller-supplied one.
pub(crate) fn env_client_with_limits(limits: RunLimits) -> Result<GatewayClient> {
    GatewayClient::from_env()
        .map(|client| client.with_request_limits(limits.timeout(), limits.response_bytes()))
        .map_err(Error::from)
}

/// How the nested `models.infer` path obtains its gateway client.
///
/// Centralizes lazy client acquisition (F5): rather than eagerly building an
/// environment client and discarding a construction failure with `.ok()`, the
/// hook carries a source and resolves it on the FIRST attempted inference, so a
/// concrete construction error (for example a missing gateway key) is surfaced
/// at infer time instead of being silently swallowed.
#[derive(Clone)]
pub(crate) enum GatewaySource {
    /// A client the caller supplied or the run already built.
    Ready(GatewayClient),
    /// Build from the environment with the run's limits on first use.
    Env(RunLimits),
}

impl GatewaySource {
    /// Chooses a ready client when one exists, else an environment source.
    pub(crate) fn from_optional(client: Option<GatewayClient>, limits: RunLimits) -> GatewaySource {
        client.map_or(GatewaySource::Env(limits), GatewaySource::Ready)
    }

    /// Resolves the source to a concrete client, preserving a build error.
    pub(crate) fn resolve(&self) -> Result<GatewayClient> {
        match self {
            GatewaySource::Ready(client) => Ok(client.clone()),
            GatewaySource::Env(limits) => env_client_with_limits(*limits),
        }
    }
}
