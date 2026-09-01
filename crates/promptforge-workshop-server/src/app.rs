//! Shared handler state and the composition root that assembles the
//! per-feature routers into the workshop server.

use std::sync::Arc;

use axum::Router;

use promptforge_progress::ProgressHub;

use crate::backoff::ReconnectBackoff;
use crate::catalog::CatalogBus;
use crate::config::Config;
use crate::deadline::{DEFAULT_DEADLINE, with_deadline};
use crate::gateway::{GatewayClient, GatewayError};
use crate::heartbeat::GatewayHealth;
use crate::menu::MenuBus;
use crate::protocol::Activity;
use crate::push::Push;
use crate::routes;
use crate::session_agents::{AgentSessions, SessionHost};
use crate::status::StatusBus;
use crate::tape::{Tape, TapeError};
use crate::workspace::Workspace;

/// Address the server binds to when no override is given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7910";

/// Shared handler state: the authenticated gateway client, the session
/// tape, the status, catalog, and menu buses, the process progress hub,
/// the hosted workspace state, and the agent-session registry.
#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) gateway: GatewayClient,
    pub(crate) tape: Arc<Tape>,
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
    /// Builds shared state from the loaded configuration.
    ///
    /// # Errors
    /// Returns [`StateError::Gateway`] if the HTTP client cannot be built
    /// and [`StateError::Tape`] if the session tape cannot be opened.
    pub fn new(config: &Config) -> Result<Self, StateError> {
        let status = StatusBus::new();
        let catalog = CatalogBus::new();
        // The per-profile model memory lives beside the tape file; a bad
        // or missing memory file costs the memory, never startup.
        let state_dir = config.tape.path.parent();
        if let Some(dir) = state_dir {
            // A crash between an atomic write's temp file and its rename
            // orphans the temp; boot is the one moment the directory is
            // known and quiet, so it is swept here.
            crate::atomic::sweep_orphaned_temps(dir);
        }
        let menu = MenuBus::new(catalog.clone(), state_dir);
        let push = Push::new(status.clone(), catalog.clone(), menu.clone());
        // Startup phases are reported as they run; with no client connected
        // yet these land on an empty bus, ready for the first session.
        push.push_status_update(
            "Connecting to gateway",
            format!("base URL {}", config.gateway.base_url),
            Activity::General,
        );
        let gateway = GatewayClient::new(&config.gateway.base_url, &config.gateway.api_key)
            .map_err(StateError::Gateway)?;
        let tape = Tape::open(&config.tape.path).map_err(StateError::Tape)?;
        let progress = Arc::new(ProgressHub::new());
        let backoff = ReconnectBackoff::new();
        let workspace = Workspace::new();
        let agents = AgentSessions::new(
            config.agents.path.clone(),
            config.server.state_dir.join("sessions"),
            crate::session_agents::model_client(&config.gateway.base_url, &config.gateway.api_key),
            SessionHost {
                push: push.clone(),
                backoff: backoff.clone(),
                menu: menu.clone(),
                workspace: workspace.clone(),
                catalog: catalog.clone(),
            },
        );
        push.push_idle();
        Ok(Self {
            gateway,
            tape: Arc::new(tape),
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

    /// The gateway client, shared with the chat WebSocket sessions.
    #[must_use]
    pub fn gateway_client(&self) -> &GatewayClient {
        &self.gateway
    }

    /// The session tape, shared with the chat WebSocket sessions.
    pub(crate) fn tape(&self) -> &Arc<Tape> {
        &self.tape
    }

    /// Shared gateway reachability, published by the heartbeat; the
    /// gateway-dependent routes read it to short-circuit while the gateway
    /// is down.
    #[must_use]
    pub fn health(&self) -> &GatewayHealth {
        &self.health
    }

    /// The shared reconnect backoff: the heartbeat draws probe delays
    /// from it while the gateway is down, and the chat paths reset it on
    /// useful work - a delivered token or a successful completion.
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

/// A shared-state construction failure: rich, init-only, and never sent
/// over the wire (the HTTP failure type is `crate::error::AppError`).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StateError {
    /// The gateway HTTP client could not be built.
    #[non_exhaustive]
    #[error("build gateway client")]
    Gateway(#[source] GatewayError),

    /// The session tape could not be opened.
    #[non_exhaustive]
    #[error("open session tape")]
    Tape(#[source] TapeError),
}

/// Returns the workshop server router with every route mounted: each
/// feature router from `crate::routes` built and merged, the workspace
/// group narrowed to the one service its handlers use. The API routes sit
/// behind the `crate::cross_site` guard; `/health` and the UI assets
/// stay outside it so the shell probe, heartbeat, and initial navigation
/// keep working. Every HTTP route carries a `crate::deadline` tier -
/// the default here, the relay tier inside `routes::chat` - and the
/// WebSocket upgrades carry none.
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
    use crate::app::AppState;
    #[cfg(test)]
    use crate::config::{AgentsConfig, Config, GatewayConfig, ServerConfig, TapeConfig};

    /// Builds a configuration pointing at `base_url`, taping to `tape_path`
    /// and anchoring the state directory beside the tape.
    #[cfg(test)]
    pub(crate) fn config_for(base_url: &str, tape_path: &Path) -> Config {
        let state_dir = tape_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Config {
            gateway: GatewayConfig {
                base_url: base_url.to_string(),
                api_key: "test-key".to_string(),
            },
            tape: TapeConfig {
                path: tape_path.to_path_buf(),
            },
            server: ServerConfig {
                state_dir,
                ..ServerConfig::default()
            },
            agents: AgentsConfig::default(),
        }
    }

    /// Builds state whose tape lives in a fresh tempdir, returned alongside
    /// so the directory outlives the test.
    #[cfg(test)]
    pub(crate) fn state_for(base_url: &str) -> (AppState, tempfile::TempDir) {
        let tape_dir = tempfile::TempDir::new().expect("tempdir");
        let config = config_for(base_url, &tape_dir.path().join("tape.jsonl"));
        let state = AppState::new(&config).expect("state builds in tests");
        (state, tape_dir)
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
    fn startup_sweeps_orphaned_temp_files_beside_the_tape() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        // Residue of a write that crashed between its temp file and its
        // rename in a previous run.
        let orphan = dir.path().join("workshop-state.json.42-7.pf-tmp");
        std::fs::write(&orphan, "partial").expect("the simulated crash residue writes");
        let config = config_for("http://127.0.0.1:1", &dir.path().join("tape.jsonl"));
        let _state = AppState::new(&config).expect("state builds");
        assert!(
            !orphan.exists(),
            "state construction sweeps orphaned temp files from the state directory"
        );
    }

    #[test]
    fn unopenable_tape_path_fails_state_construction() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let config = config_for(
            "http://127.0.0.1:1",
            &dir.path().join("missing").join("tape.jsonl"),
        );
        let err = AppState::new(&config).expect_err("an unopenable tape must fail");
        assert!(
            matches!(err, StateError::Tape(_)),
            "expected Tape, got {err:?}"
        );
    }
}
