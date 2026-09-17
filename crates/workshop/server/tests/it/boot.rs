//! Boot composition: a subsystem whose `register` call is missing fails
//! state construction at boot, naming the absent contribution, instead
//! of panicking later at first use.

use workshop_server::fixtures::{Omit, state_with_gateway_omitting};
use workshop_server::{AgentsConfig, Config, GatewayConfig, ResolvedGateway, ServerConfig};

/// A config against a stub gateway address, its state directory a fresh
/// tempdir.
fn config_for(state_dir: &std::path::Path) -> Config {
    Config {
        gateway: GatewayConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            state_dir: state_dir.to_path_buf(),
            ..ServerConfig::default()
        },
        agents: AgentsConfig::default(),
    }
}

#[test]
fn a_missing_required_contribution_fails_boot_naming_it() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let config = config_for(state_dir.path());
    let gateway = ResolvedGateway::from_config(&config.gateway);
    let error = state_with_gateway_omitting(&config, &gateway, Omit::Menu)
        .expect_err("boot fails when the menu subsystem never registers");
    assert!(
        error.to_string().contains("MenuHandles"),
        "the failure names the missing contribution: {error}"
    );
}

#[test]
fn a_complete_composition_boots() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let config = config_for(state_dir.path());
    let gateway = ResolvedGateway::from_config(&config.gateway);
    workshop_server::fixtures::state_with_gateway(&config, &gateway)
        .expect("every subsystem registered: boot succeeds");
}
