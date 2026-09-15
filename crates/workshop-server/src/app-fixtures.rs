//! Shared fixtures for the router tests here and in the
//! [`crate::routes`] feature modules: state construction against a stub
//! gateway address and the small helpers every route test leans on.
//! [`spawn_gateway`] is additionally re-exported to the
//! integration-test binary through the `test-fixtures` feature the
//! crate's own dev-dependency enables.
// An `allow` rather than an `expect`: whether the lint fires here depends
// on the build's cfg permutation (clippy suppresses expect_used inside
// test-cfg'd code on its own), so an expectation would be unfulfilled in
// some builds and fail the -D warnings gate.
#![allow(
    clippy::expect_used,
    reason = "test fixtures fail by panicking with the invariant named"
)]

#[cfg(test)]
use std::path::Path;

use axum::Router;
#[cfg(test)]
use axum::body::to_bytes;
#[cfg(test)]
use axum::response::Response;

#[cfg(test)]
use crate::app::{AppState, state_with_gateway};
#[cfg(test)]
use workshop_support::{AgentsConfig, Config, GatewayConfig, ServerConfig};

/// Builds a configuration pointing at `base_url`, anchoring the state
/// directory at `state_dir`.
#[cfg(test)]
pub(crate) fn config_for(base_url: &str, state_dir: &Path) -> Config {
    Config {
        gateway: GatewayConfig {
            base_url: base_url.to_string(),
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            state_dir: state_dir.to_path_buf(),
            ..ServerConfig::default()
        },
        agents: AgentsConfig::default(),
    }
}

/// Builds state whose state directory is a fresh tempdir, returned
/// alongside so the directory outlives the test. Discovery is
/// bypassed: a test never consults the real run directory.
#[cfg(test)]
pub(crate) fn state_for(base_url: &str) -> (AppState, tempfile::TempDir) {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let config = config_for(base_url, state_dir.path());
    let gateway = crate::resolve::ResolvedGateway::from_config(&config.gateway);
    let state = state_with_gateway(&config, &gateway).expect("state builds in tests");
    (state, state_dir)
}

/// Collects a response body already buffered in memory.
#[cfg(test)]
pub(crate) async fn body_bytes(response: Response) -> axum::body::Bytes {
    to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already")
}

/// Binds `app` as a mock gateway on a free loopback port and returns its
/// base URL.
///
/// # Panics
/// Panics when the loopback bind fails or the bound address cannot be
/// read.
pub async fn spawn_gateway(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock gateway");
    let addr = listener.local_addr().expect("mock gateway address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock gateway serves");
    });
    format!("http://{addr}")
}
