//! Integration-test seams that exercise Workshop behavior in-process.

#[cfg(feature = "test-fixtures")]
pub use crate::app::state_with_gateway_and_restart_bound;
pub use crate::app::{Omit, state_with_gateway, state_with_gateway_omitting};
pub use crate::catalog::CatalogBus;
pub use crate::heartbeat::{GatewayHealth, Heartbeat};
pub use crate::menu::{MenuBus, MenuRefusal};
pub use crate::push::Push;
pub use crate::status::StatusBus;
pub use workshop_protocol::{Activity, Severity, StatusBarUpdate};
pub use workshop_support::ReconnectBackoff;

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
/// Returns [`crate::GatewayPublicationError::Build`] when the fixture client
/// cannot initialize, or
/// [`crate::GatewayPublicationError::PublicationClosed`] after teardown.
#[cfg(feature = "test-fixtures")]
pub fn replace_gateway(
    updater: &crate::GatewayUpdater,
    base_url: &str,
    api_key: &str,
) -> Result<(), crate::GatewayPublicationError> {
    updater.replace_fixture(base_url, api_key)
}

/// Starts the sessions subsystem's bindings forwarder over fixture state:
/// the registered background task that pushes every gateway, catalog,
/// and menu replacement through the harness's public API. The server
/// spawns it with serving; a test that binds the router directly has no
/// serving loop, so it spawns the forwarder here. The task ends with the
/// state.
#[cfg(feature = "test-fixtures")]
pub fn spawn_bindings_forwarder(state: &crate::AppState) {
    drop(tokio::spawn(crate::agents::forward_bindings(
        state.registry().clone(),
    )));
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

/// The named child-process half of [`ValidatedGateway`]: the fixture
/// spawns a copy of this test binary with this test's name, so the name
/// must stay in sync with the `spawn_in` call sites.
#[cfg(test)]
#[test]
#[ignore = "runs only as a named child process"]
fn validated_gateway_fixture_process() {
    run_validated_gateway_fixture_process();
}

/// Spawns a Workshop test server against the explicit configured Gateway.
#[cfg(feature = "test-fixtures")]
pub fn spawn(config: crate::Config) -> Result<crate::ServerHandle, crate::SpawnError> {
    crate::serve::spawn_resolved(config)
}
