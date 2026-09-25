//! Desktop-app-facing surface tests: every re-export is named and the fixtures feature forwards the seams.

use super::*;

/// The unqualified type name of `T`, so the assertions read as the
/// shipping surface reads.
fn short_name<T>() -> &'static str {
    let name = std::any::type_name::<T>();
    name.rsplit("::").next().unwrap_or(name)
}

/// Compile-level check: `spawn` resolves and has the server's signature.
fn spawn_signature(start: fn(Config) -> Result<ServerHandle, SpawnError>) {
    let _ = start;
}

#[test]
fn the_desktop_app_facing_surface_names_every_re_export() {
    // Configuration.
    assert_eq!(short_name::<AgentsConfig>(), "AgentsConfig");
    assert_eq!(short_name::<Config>(), "Config");
    assert_eq!(short_name::<GatewayConfig>(), "GatewayConfig");
    assert_eq!(short_name::<ServerConfig>(), "ServerConfig");
    // The in-process server lifecycle.
    spawn_signature(spawn);
    assert_eq!(short_name::<ServerHandle>(), "ServerHandle");
    assert_eq!(short_name::<SpawnError>(), "SpawnError");
    assert_eq!(short_name::<Termination>(), "Termination");
    // The Gateway publication seam.
    assert_eq!(short_name::<GatewayUpdater>(), "GatewayUpdater");
    assert_eq!(
        short_name::<GatewayPublicationError>(),
        "GatewayPublicationError"
    );
}

#[test]
fn the_test_fixtures_feature_forwards_the_server_seams() {
    spawn_signature(fixtures::spawn);
}
