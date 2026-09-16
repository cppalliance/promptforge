//! The shutdown-request half of quit, against a stubbed gateway: the
//! sidecar snapshot receives the authenticated `/shutdown`, a configured
//! gateway is left running (no shutdown authority), and a missing server
//! is a no-op.

use std::time::Duration;

use super::request_gateway_shutdown;

/// How long a test waits for the stubbed gateway to report the shutdown.
const SHUTDOWN_OBSERVE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a test waits to prove no shutdown was sent.
const NO_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);

#[test]
#[ignore = "runs only as a named child process"]
fn validated_gateway_fixture_process() {
    workshop_server_api::fixtures::run_validated_gateway_fixture_process();
}

/// Starts the shared named-process Gateway stub in this test binary.
fn stubbed_gateway(expected_key: &str) -> workshop_server_api::fixtures::ValidatedGateway {
    workshop_server_api::fixtures::ValidatedGateway::spawn_in(
        expected_key,
        "quit::tests::validated_gateway_fixture_process",
    )
}

/// Spawns a Workshop fixture server configured against the gateway at
/// `port` - an explicit configuration, which alone grants no shutdown
/// authority.
fn workshop_server(
    port: u16,
    api_key: &str,
) -> (tempfile::TempDir, workshop_server_api::ServerHandle) {
    let state_dir = tempfile::TempDir::new().expect("create Workshop state directory");
    let server = workshop_server_api::fixtures::spawn(workshop_server_api::Config {
        gateway: workshop_server_api::GatewayConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            api_key: api_key.to_owned(),
        },
        server: workshop_server_api::ServerConfig {
            bind: "127.0.0.1:0".to_owned(),
            open_browser: false,
            state_dir: state_dir.path().to_owned(),
        },
        agents: workshop_server_api::AgentsConfig::default(),
    })
    .expect("spawn Workshop fixture");
    (state_dir, server)
}

#[test]
fn quit_requests_shutdown_from_the_current_sidecar_snapshot() {
    let mut gateway = stubbed_gateway("quit-key");
    let (_state_dir, server) = workshop_server(gateway.port(), "quit-key");
    let updater = server.gateway_updater();
    let sidecar = gateway.validate("quit-key", 1_778_000_001, "2026-09-15T18:00:01Z");
    updater
        .replace_sidecar(&sidecar)
        .expect("the sidecar identity publishes");

    request_gateway_shutdown(Some(updater));

    assert!(
        gateway.received_shutdown(SHUTDOWN_OBSERVE_TIMEOUT),
        "quit posts the authenticated shutdown to the current sidecar"
    );
    server.shutdown().expect("stop the Workshop fixture");
}

#[test]
fn quit_leaves_a_configured_gateway_running() {
    let mut gateway = stubbed_gateway("lan-key");
    let (_state_dir, server) = workshop_server(gateway.port(), "lan-key");

    request_gateway_shutdown(Some(server.gateway_updater()));

    assert!(
        !gateway.received_shutdown(NO_SHUTDOWN_TIMEOUT),
        "an explicitly configured gateway grants no shutdown authority"
    );
    server.shutdown().expect("stop the Workshop fixture");
}

#[test]
fn quit_without_a_server_requests_nothing() {
    request_gateway_shutdown(None);
}
