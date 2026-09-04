//! Shared handler state and the composition root that assembles the
//! per-feature routers into the workshop server.

use std::sync::Arc;

use axum::Router;

use shared_progress::ProgressHub;

use crate::backoff::ReconnectBackoff;
use crate::catalog::CatalogBus;
use crate::config::Config;
use crate::deadline::{DEFAULT_DEADLINE, with_deadline};
use crate::gateway::{GatewayClient, GatewayError};
use crate::heartbeat::GatewayHealth;
use crate::menu::MenuBus;
use crate::push::Push;
use crate::resolve::ResolvedGateway;
use crate::routes;
use crate::session_agents::{AgentSessions, SessionHost};
use crate::status::StatusBus;
use crate::workspace::Workspace;

/// Address the server binds to when no override is given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7910";

/// Shared handler state: the authenticated gateway client, the status,
/// catalog, and menu buses, the process progress hub, the hosted
/// workspace state, and the agent-session registry.
#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) gateway: GatewayClient,
    pub(crate) status: StatusBus,
    pub(crate) progress: Arc<ProgressHub>,
    pub(crate) health: GatewayHealth,
    pub(crate) backoff: ReconnectBackoff,
    pub(crate) catalog: CatalogBus,
    pub(crate) menu: MenuBus,
    pub(crate) workspace: Workspace,
    pub(crate) agents: AgentSessions,
}

impl AppState {
    /// Builds shared state from the loaded configuration, resolving the
    /// gateway endpoint first: a live connection file in the run
    /// directory wins over explicit `[gateway]` config.
    ///
    /// # Errors
    /// Returns [`StateError::Resolution`] when no live connection file
    /// exists and the config carries no explicit gateway, and
    /// [`StateError::Gateway`] if the HTTP client cannot be built.
    pub fn new(config: &Config) -> Result<Self, StateError> {
        let gateway = crate::resolve::resolve(&config.gateway).map_err(StateError::Resolution)?;
        state_with_gateway(config, &gateway)
    }

    /// The status bus, which every `/ws` session subscribes to so it can
    /// forward updates; producers report through [`AppState::push`].
    #[must_use]
    pub fn status(&self) -> StatusBus {
        self.status.clone()
    }

    /// The process progress hub: operations with bounded lifetimes attach
    /// trees to it as they run, and the renderer task (spawned with the
    /// server) turns its snapshots into the status bar's progress
    /// indicator.
    pub(crate) fn progress(&self) -> &Arc<ProgressHub> {
        &self.progress
    }

    /// The push facade over the status, catalog, and menu buses, held by
    /// every subsystem that reports what happened.
    #[must_use]
    pub fn push(&self) -> Push {
        Push::new(self.status.clone(), self.catalog.clone(), self.menu.clone())
    }

    /// The gateway client, shared with the heartbeat and the relay routes.
    #[must_use]
    pub fn gateway_client(&self) -> &GatewayClient {
        &self.gateway
    }

    /// Shared gateway reachability, published by the heartbeat; the
    /// gateway-dependent routes read it to short-circuit while the gateway
    /// is down.
    #[must_use]
    pub fn health(&self) -> &GatewayHealth {
        &self.health
    }

    /// The shared reconnect backoff: the heartbeat draws probe delays
    /// from it while the gateway is down, and the agent sessions reset it
    /// on useful work - a completed model reply.
    #[must_use]
    pub fn backoff(&self) -> &ReconnectBackoff {
        &self.backoff
    }

    /// The catalog bus, which the heartbeat publishes the refreshed model
    /// catalog to on a gateway reconnect and every `/ws` session forwards
    /// from.
    #[must_use]
    pub fn catalog(&self) -> CatalogBus {
        self.catalog.clone()
    }

    /// The menu bus, whose workbench snapshots every `/ws` session
    /// forwards and whose mutators the session's menu events drive.
    #[must_use]
    pub fn menu(&self) -> &MenuBus {
        &self.menu
    }

    /// The confined workspace, shared with the `/workspace/*` handlers;
    /// grants registered through `POST /workspace/grant` are visible to
    /// every clone immediately.
    pub(crate) fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The agent-session registry: discovery, launch, and the running
    /// sessions behind the `/agents/ws` socket. Sessions outlive
    /// sockets, so an embedding host ends one through
    /// [`AgentSessions::close`].
    #[must_use]
    pub fn agents(&self) -> &AgentSessions {
        &self.agents
    }
}

/// Builds shared state against an already-resolved gateway endpoint: the
/// construction phase a host holding its own endpoint enters directly,
/// skipping connection-file discovery.
///
/// # Errors
/// Returns [`StateError::Gateway`] if the HTTP client cannot be built.
pub fn state_with_gateway(
    config: &Config,
    gateway: &ResolvedGateway,
) -> Result<AppState, StateError> {
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    // The per-profile model memory lives in the state directory; a bad
    // or missing memory file costs the memory, never startup.
    let state_dir = &config.server.state_dir;
    // A crash between an atomic write's temp file and its rename
    // orphans the temp; boot is the one moment the directory is
    // known and quiet, so it is swept here.
    crate::atomic::sweep_orphaned_temps(state_dir);
    let menu = MenuBus::new(catalog.clone(), Some(state_dir));
    let push = Push::new(status.clone(), catalog.clone(), menu.clone());
    // Startup phases are reported as they run; with no client connected
    // yet these land on an empty bus, ready for the first session.
    crate::resolve::report(gateway, &push);
    let client =
        GatewayClient::new(gateway.base_url(), gateway.api_key()).map_err(StateError::Gateway)?;
    let progress = Arc::new(ProgressHub::new());
    let backoff = ReconnectBackoff::new();
    let workspace = Workspace::new();
    let agents = AgentSessions::new(
        config.agents.path.clone(),
        config.server.state_dir.join("sessions"),
        crate::session_agents::model_client(gateway.base_url(), gateway.api_key()),
        SessionHost {
            push: push.clone(),
            backoff: backoff.clone(),
            menu: menu.clone(),
            workspace: workspace.clone(),
            catalog: catalog.clone(),
        },
    );
    push.push_idle();
    Ok(AppState {
        gateway: client,
        status,
        progress,
        health: GatewayHealth::new(),
        backoff,
        catalog,
        menu,
        workspace,
        agents,
    })
}

/// A shared-state construction failure: rich, init-only, and never sent
/// over the wire (the HTTP failure type is `crate::error::AppError`).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StateError {
    /// The gateway HTTP client could not be built.
    #[non_exhaustive]
    #[error("build gateway client")]
    Gateway(#[source] GatewayError),

    /// No gateway endpoint could be resolved: no live connection file and
    /// no explicit `[gateway]` config.
    #[non_exhaustive]
    #[error("resolve the gateway endpoint")]
    Resolution(#[source] crate::resolve::ResolveError),
}

/// Returns the workshop server router with every route mounted: each
/// feature router from `crate::routes` built and merged, the workspace
/// group narrowed to the one service its handlers use. The API routes sit
/// behind the `crate::cross_site` guard; `/health` and the UI assets
/// stay outside it so the shell probe, heartbeat, and initial navigation
/// keep working. Every HTTP route carries a `crate::deadline` tier -
/// the default here, the relay tier inside `routes::chat` - and the
/// WebSocket upgrades carry none. Every response carries the
/// `crate::csp` policy: the shell's webview loads the UI as an External
/// origin, so the server sets the page's Content-Security-Policy.
pub fn router(state: AppState) -> Router {
    let workspace = state.workspace().clone();
    let api = Router::new()
        .merge(routes::chat::routes(state.clone()))
        .merge(crate::session_agents::socket::routes(state.clone()))
        .merge(routes::gateway_config::routes(state))
        .merge(with_deadline(
            routes::workspace::routes(workspace),
            DEFAULT_DEADLINE,
        ))
        .layer(axum::middleware::from_fn(crate::cross_site::guard));
    Router::new()
        .merge(with_deadline(routes::assets::routes(), DEFAULT_DEADLINE))
        .merge(with_deadline(routes::health::routes(), DEFAULT_DEADLINE))
        .merge(api)
        // The outermost layer on the server's own routes: every response
        // carries the CSP, error envelopes included, so the shell's
        // External-origin webview runs under the policy no matter which
        // route answered. Routes a host merges through `spawn_with_routes`
        // are composed after this layer and sit outside it.
        .layer(axum::middleware::from_fn(crate::csp::header))
}

/// Shared fixtures for the router tests here and in [`crate::relay`]
/// and the [`crate::routes`] feature modules: state
/// construction against a stub gateway address and
/// the small helpers every route test leans on. [`fixtures::spawn_gateway`]
/// is additionally re-exported to the integration-test binary through the
/// `test-fixtures` feature the crate's own dev-dependency enables.
// An `allow` rather than an `expect`: whether the lint fires here depends
// on the build's cfg permutation (clippy suppresses expect_used inside
// test-cfg'd code on its own), so an expectation would be unfulfilled in
// some builds and fail the -D warnings gate.
#[cfg(any(test, feature = "test-fixtures"))]
#[allow(
    clippy::expect_used,
    reason = "test fixtures fail by panicking with the invariant named"
)]
pub(crate) mod fixtures {
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
    use crate::config::{AgentsConfig, Config, GatewayConfig, ServerConfig};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::{HeaderMap, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    use super::fixtures::{config_for, spawn_gateway};

    /// Reports whether the request carried an `Authorization` header, so
    /// the client tests can observe what was sent.
    async fn mock_auth_probe(headers: HeaderMap) -> Response {
        let body = if headers.contains_key(header::AUTHORIZATION) {
            "auth"
        } else {
            "no-auth"
        };
        ([(header::CONTENT_TYPE, "text/plain")], body).into_response()
    }

    #[tokio::test]
    async fn empty_api_key_sends_no_authorization_header() {
        let base_url = spawn_gateway(Router::new().route("/v1/models", get(mock_auth_probe))).await;
        let anonymous = GatewayClient::new(&base_url, "").expect("client builds");
        let response = anonymous.list_models().await.expect("request completes");
        assert_eq!(response.body, b"no-auth", "empty key sends no header");

        let keyed = GatewayClient::new(&base_url, "test-key").expect("client builds");
        let response = keyed.list_models().await.expect("request completes");
        assert_eq!(response.body, b"auth", "a set key still authenticates");
    }

    #[test]
    fn default_bind_is_loopback_port_7910() {
        assert_eq!(DEFAULT_ADDR, "127.0.0.1:7910");
    }

    #[test]
    fn startup_sweeps_orphaned_temp_files_from_the_state_directory() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        // Residue of a write that crashed between its temp file and its
        // rename in a previous run.
        let orphan = dir.path().join("workshop-state.json.42-7.pf-tmp");
        std::fs::write(&orphan, "partial").expect("the simulated crash residue writes");
        let config = config_for("http://127.0.0.1:1", dir.path());
        let gateway = ResolvedGateway::from_config(&config.gateway);
        let _state = state_with_gateway(&config, &gateway).expect("state builds");
        assert!(
            !orphan.exists(),
            "state construction sweeps orphaned temp files from the state directory"
        );
    }
}
