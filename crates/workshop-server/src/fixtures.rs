//! Integration-test seams that exercise Workshop behavior in-process.

pub use crate::app::state_with_gateway;
pub use crate::backoff::ReconnectBackoff;
pub use crate::catalog::CatalogBus;
pub use crate::heartbeat::{GatewayHealth, Heartbeat};
pub use crate::menu::{MenuBus, MenuRefusal};
pub use crate::protocol::{Activity, Progress, Severity, StatusBarUpdate};
pub use crate::push::Push;
pub use crate::status::StatusBus;

#[cfg(feature = "test-fixtures")]
pub use crate::app::fixtures::spawn_gateway;
#[cfg(feature = "test-fixtures")]
pub use crate::test_gateway::{ValidatedGateway, run_validated_gateway_fixture_process};

/// Returns the host-only Gateway publisher from fixture state.
#[cfg(feature = "test-fixtures")]
#[must_use]
pub fn gateway_updater(state: &crate::AppState) -> crate::GatewayUpdater {
    state.gateway_updater()
}

/// Replaces a configured Gateway fixture without creating a production
/// sidecar capability.
///
/// # Errors
/// Returns [`crate::GatewayError::Build`] when the fixture client cannot
/// initialize.
#[cfg(feature = "test-fixtures")]
pub fn replace_gateway(
    updater: &crate::GatewayUpdater,
    base_url: &str,
    api_key: &str,
) -> Result<(), crate::GatewayError> {
    updater.replace_fixture(base_url, api_key)
}

/// Starts a heartbeat around a fixture Gateway client.
#[must_use]
pub fn spawn_heartbeat(
    client: crate::GatewayClient,
    push: crate::Push,
    health: GatewayHealth,
    interval: std::time::Duration,
    backoff: ReconnectBackoff,
) -> Heartbeat {
    crate::heartbeat::spawn(
        crate::gateway_binding::GatewayBinding::from_client(client),
        push,
        health,
        interval,
        backoff,
    )
}

/// Spawns a Workshop test server against the explicit configured Gateway.
#[cfg(feature = "test-fixtures")]
pub fn spawn(config: crate::Config) -> Result<crate::ServerHandle, crate::SpawnError> {
    crate::serve::spawn_resolved(config)
}
