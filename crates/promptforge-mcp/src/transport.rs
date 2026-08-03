//! The two transports the server is reachable on, and the bearer check over
//! one of them.
//!
//! Streamable HTTP is the networked case: the MCP endpoint is nested at `/mcp`
//! with a 15-second SSE keep-alive, because a progress notification rides the
//! stream the `tools/call` POST opened and an idle proxy must not close it. The
//! shared bearer is checked once per HTTP request rather than once per MCP
//! session, so a rotated token refuses an established session's next call. A
//! request that presents no `Bearer` credential is refused before any
//! comparison happens, and the configuration refuses an empty token, so the two
//! defences hold independently.
//!
//! `/healthz` is registered *after* the bearer layer, which is the whole of its
//! exemption: the layer never sees the route, so nothing inside the middleware
//! compares a path and no later route addition can accidentally inherit or lose
//! the check. A route added before the layer is guarded; one added after it is
//! not, and that is the only rule to keep in mind.
//!
//! stdio is the local case. It binds no port, constructs no auth layer, and
//! therefore reads no token: a harness that spawned this process already has
//! whatever authority the process has.

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rmcp::ServiceExt;
use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;

use crate::catalog::CatalogHandle;
use crate::config::{Config, Secret};
use crate::error::ServeError;
use crate::retrieval::Retrieval;
use crate::server::PromptForgeServer;
use crate::watch::Sessions;

/// Where the MCP endpoint is nested. Everything under it is behind the bearer.
pub const MCP_PATH: &str = "/mcp";

/// The liveness path. Registered after the bearer layer, so it is exempt by
/// where it sits rather than by anything the middleware tests.
pub const HEALTHZ_PATH: &str = "/healthz";

/// How long an idle SSE stream may go without a ping. A run reports progress on
/// the stream its call opened, and a run that thinks for longer than this
/// between sections would otherwise look dead to a proxy.
const SSE_KEEP_ALIVE: Duration = Duration::from_secs(15);

/// Builds the axum router: the MCP endpoint behind the bearer check, then the
/// liveness probe outside it.
///
/// The order of the three calls is load-bearing. `nest_service` registers
/// `/mcp`, `layer` wraps everything registered so far, and `/healthz` is added
/// afterwards and so is never wrapped. Add a new authenticated route before the
/// `layer` call and a new public one after it.
///
/// # Examples
/// ```
/// # use std::sync::Arc;
/// # use promptforge_mcp::{
/// #     Catalog, CatalogHandle, Config, OnBroken, PromptForgeServer, Retrieval, Sessions,
/// # };
/// # fn demo(config: Config, catalog: Catalog) {
/// let token = Arc::new(config.server.token.clone());
/// let config = Arc::new(config);
/// let catalog = Arc::new(CatalogHandle::new(catalog));
/// let server = PromptForgeServer::new(
///     config,
///     catalog,
///     Arc::new(Sessions::new()),
///     Arc::new(Retrieval::idle()),
/// );
/// let router = promptforge_mcp::build_router(server, token);
/// # let _ = router;
/// # }
/// ```
pub fn build_router(server: PromptForgeServer, token: Arc<Secret>) -> Router {
    // One handler, cloned per session: the clone shares the configuration, the
    // catalog, and the run registry, which is what lets a run started in one
    // session be collected by `check_run` from another.
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        streamable_config(),
    );
    Router::new()
        .nest_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(token, require_bearer))
        .route(HEALTHZ_PATH, get(healthz))
}

/// The streamable-HTTP settings: sessions on, because a progress notification
/// rides the SSE stream the call opened, and a keep-alive under every proxy's
/// idle timeout.
fn streamable_config() -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_sse_keep_alive(Some(SSE_KEEP_ALIVE))
        .with_legacy_session_mode(true)
}

/// Serves the streamable-HTTP transport on `[server].bind` until the process
/// is stopped.
///
/// # Errors
/// Returns [`ServeError::Bind`] if the configured address cannot be bound and
/// [`ServeError::Http`] if the accept loop stops with an error.
pub async fn serve_http(
    config: Arc<Config>,
    catalog: Arc<CatalogHandle>,
    sessions: Arc<Sessions>,
    retrieval: Arc<Retrieval>,
) -> Result<(), ServeError> {
    let bind = config.server.bind;
    let token = Arc::new(config.server.token.clone());
    let server = PromptForgeServer::new(Arc::clone(&config), catalog, sessions, retrieval);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|source| ServeError::Bind { addr: bind, source })?;
    tracing::info!("promptforge-mcp serving on http://{bind}{MCP_PATH}");
    axum::serve(listener, build_router(server, token))
        .await
        .map_err(|source| ServeError::Http { source })
}

/// Serves the stdio transport on this process's standard input and output
/// until the peer closes it.
///
/// No port is bound and no token is checked: the harness that spawned this
/// process is the only thing that can talk to it. `[server].bind` and
/// `[server].token` are read by nothing here, and a configuration that sets
/// them is logged as ignored rather than silently obeyed or refused.
///
/// # Errors
/// Returns [`ServeError::Stdio`] if the MCP handshake does not complete or the
/// session ends abnormally.
pub async fn serve_stdio(
    config: Arc<Config>,
    catalog: Arc<CatalogHandle>,
    sessions: Arc<Sessions>,
    retrieval: Arc<Retrieval>,
) -> Result<(), ServeError> {
    tracing::info!(
        "promptforge-mcp serving on stdio; [server].bind ({}) and [server].token are not used on this transport",
        config.server.bind
    );
    let server = PromptForgeServer::new(config, catalog, sessions, retrieval);
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| ServeError::Stdio(error.to_string()))?;
    running
        .waiting()
        .await
        .map_err(|error| ServeError::Stdio(error.to_string()))?;
    Ok(())
}

/// The liveness probe: unauthenticated, and 200 for as long as the process
/// serves.
async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "serving" }))
}

/// Checks the shared bearer on one HTTP request.
///
/// Per request rather than per session, so rotating the token refuses the very
/// next call on a session that already initialized. A refusal carries
/// `WWW-Authenticate: Bearer`, which is what tells a client the scheme it
/// failed rather than that the endpoint is gone.
async fn require_bearer(
    State(token): State<Arc<Secret>>,
    request: Request,
    next: Next,
) -> Response {
    // A missing header, or one whose scheme is not `Bearer`, is refused here
    // rather than compared. Falling through with the empty string is what let an
    // empty configured token authenticate a caller presenting nothing, and the
    // configuration refuses an empty token as well, so neither a typo nor a
    // comparison bug can open the surface alone.
    let Some(presented) = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return unauthorized();
    };
    if constant_time_eq(presented.as_bytes(), token.expose().as_bytes()) {
        next.run(request).await
    } else {
        unauthorized()
    }
}

/// A refusal naming the scheme the caller should have used.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, "Bearer")],
        "unauthorized",
    )
        .into_response()
}

/// Length-checked byte comparison whose content pass is constant-time.
///
/// The lengths are compared first and an unequal pair returns at once, so the
/// configured token's length is readable from how long the check took. What the
/// constant-time content pass hides is the position of the first differing byte,
/// which is what a byte-at-a-time search of the token would need.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}
