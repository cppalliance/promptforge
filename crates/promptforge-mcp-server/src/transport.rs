//! The two transports the server is reachable on, and the bearer check over
//! one of them.
//!
//! Streamable HTTP is the networked case: the MCP endpoint is nested at `/mcp`
//! with a 15-second SSE keep-alive, because a progress notification rides the
//! stream the `tools/call` POST opened and an idle proxy must not close it. The
//! shared bearer is checked once per HTTP request rather than once per MCP
//! session, so a caller whose token no longer matches the one the server was
//! started with is refused on its very next call even on a session that already
//! initialized; the token itself is fixed for the life of the server, so this
//! is per-request re-authentication rather than live rotation of the secret. A
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
//! whatever authority the process has. That is why `[server].token` is
//! optional in the file and required here: the transport that checks it
//! refuses to bind without one, and the transport that never reads it boots
//! without one.

mod stdio;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::net::SocketAddr;
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
use tokio_util::sync::CancellationToken;

use crate::catalog::CatalogHandle;
use crate::config::{Config, Secret};
use crate::error::ServeError;
use crate::server::PromptForgeServer;

/// Where the MCP endpoint is nested. Everything under it is behind the bearer.
pub(crate) const MCP_PATH: &str = "/mcp";

/// The liveness path. Registered after the bearer layer, so it is exempt by
/// where it sits rather than by anything the middleware tests.
pub(crate) const HEALTHZ_PATH: &str = "/healthz";

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
/// `cancellation` is the streamable-HTTP transport's own shutdown handle:
/// cancelling it closes the SSE streams the transport holds open, which is what
/// lets a graceful drain finish rather than wait on an idle keep-alive.
pub(crate) fn build_router(
    server: PromptForgeServer,
    token: Arc<Secret>,
    cancellation: CancellationToken,
    allowed_hosts: Vec<String>,
) -> Router {
    // One handler, cloned per session: the clone shares the configuration, the
    // catalog, and the run registry, which is what lets a run started in one
    // session be collected by `check_run` from another.
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        streamable_config(cancellation, allowed_hosts),
    );
    Router::new()
        .nest_service(MCP_PATH, service)
        .layer(middleware::from_fn_with_state(token, require_bearer))
        .route(HEALTHZ_PATH, get(healthz))
}

/// The streamable-HTTP settings: sessions on, because a progress notification
/// rides the SSE stream the call opened, a keep-alive under every proxy's idle
/// timeout, the caller's `cancellation` token so a shutdown closes the
/// transport's streams instead of leaving them for the keep-alive to outlive,
/// and the `allowed_hosts` the `Host` header is validated against so the bound
/// socket cannot be reached under a name the operator did not intend.
fn streamable_config(
    cancellation: CancellationToken,
    allowed_hosts: Vec<String>,
) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_sse_keep_alive(Some(SSE_KEEP_ALIVE))
        .with_legacy_session_mode(true)
        .with_cancellation_token(cancellation)
        .with_allowed_hosts(allowed_hosts)
}

/// The host authorities the transport validates the `Host` header against,
/// resolved from `[server].allowed_hosts` and checked against the bind.
///
/// An explicit list is honoured as given. An empty list keeps the secure
/// loopback default (`localhost`, `127.0.0.1`, `::1`) only when the bind is
/// itself loopback; a non-loopback bind with an empty list is refused, since it
/// would otherwise reject ordinary requests using the machine's DNS name and
/// serve a surface whose reachable-host policy contradicts its bind.
///
/// # Errors
/// Returns a [`ServeError`] of kind
/// [`AllowedHosts`](crate::error::ServeErrorKind::AllowedHosts) for a
/// non-loopback bind whose `[server].allowed_hosts` is empty.
fn resolve_allowed_hosts(
    bind: SocketAddr,
    configured: &[String],
) -> Result<Vec<String>, ServeError> {
    if !configured.is_empty() {
        return Ok(configured.to_vec());
    }
    if bind.ip().is_loopback() {
        return Ok(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ]);
    }
    Err(ServeError::allowed_hosts(bind))
}

/// Serves the streamable-HTTP transport on `[server].bind` until `shutdown`
/// resolves.
///
/// The caller owns the stop: when `shutdown` completes, the accept loop stops
/// taking connections, the transport's own cancellation token is tripped so its
/// open SSE streams close and in-flight calls are cancelled deterministically,
/// and the listener drains before this returns. A shutdown that never fires
/// serves until the process ends.
///
/// # Errors
/// Returns a [`ServeError`] of kind
/// [`MissingToken`](crate::error::ServeErrorKind::MissingToken) if
/// `[server].token` is absent, since this transport is the one that checks it,
/// [`Bind`](crate::error::ServeErrorKind::Bind) if the configured address
/// cannot be bound, and [`Http`](crate::error::ServeErrorKind::Http) if the
/// accept loop stops with an error.
pub(crate) async fn serve_http(
    config: Arc<Config>,
    catalog: Arc<CatalogHandle>,
    tools: Arc<crate::PreparedTools>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ServeError> {
    let bind = config.server.bind;
    // Before the socket, so a configuration with no shared bearer is refused by
    // name rather than serving an unguarded `/mcp` to whatever can reach it.
    let token = Arc::new(
        config
            .server
            .token
            .clone()
            .ok_or_else(ServeError::missing_token)?,
    );
    // Also before the socket: a non-loopback bind with no enumerated hosts is a
    // reachable-host policy that contradicts its own bind, so it is refused by
    // name rather than bound and then found unreachable by DNS name.
    let allowed_hosts = resolve_allowed_hosts(bind, &config.server.allowed_hosts)?;
    let server = PromptForgeServer::new(Arc::clone(&config), catalog, tools);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|source| ServeError::bind(bind, source))?;
    tracing::info!("promptforge-mcp-server serving on http://{bind}{MCP_PATH}");
    // One token, shared two ways: the transport holds it so it can be told to
    // close its streams, and the graceful-shutdown future trips it once the
    // caller's signal fires, right before Axum stops accepting.
    let cancellation = CancellationToken::new();
    let router = build_router(server, token, cancellation.clone(), allowed_hosts);
    let graceful = async move {
        shutdown.await;
        cancellation.cancel();
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(graceful)
        .await
        .map_err(ServeError::http)
}

/// Serves the stdio transport on this process's standard input and output
/// until the peer closes it or `shutdown` resolves.
///
/// No port is bound and no token is checked: the harness that spawned this
/// process is the only thing that can talk to it. `[server].bind` and
/// `[server].token` are read by nothing here, so a configuration that sets
/// either is logged as ignored rather than silently obeyed, and one that omits
/// the token serves anyway.
///
/// The read side is capped at a documented maximum line length, so a peer that
/// sends a line without a newline costs a bounded buffer rather than the
/// process.
///
/// # Errors
/// Returns a [`ServeError`] of kind
/// [`Stdio`](crate::error::ServeErrorKind::Stdio) if the MCP handshake does not
/// complete or the session ends abnormally.
pub(crate) async fn serve_stdio(
    config: Arc<Config>,
    catalog: Arc<CatalogHandle>,
    tools: Arc<crate::PreparedTools>,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), ServeError> {
    tracing::info!(
        "promptforge-mcp-server serving on stdio; [server].bind ({}) and [server].token are not used on this transport",
        config.server.bind
    );
    let server = PromptForgeServer::new(config, catalog, tools);
    let (stdin, stdout) = rmcp::transport::stdio();
    serve_stdio_on(server, stdin, stdout, shutdown).await
}

/// Runs one stdio session over `read`/`write` until the peer closes it or
/// `shutdown` resolves, then closes cleanly.
///
/// Split out from [`serve_stdio`] so a test can drive a whole session over an
/// in-memory pipe rather than the process's real standard streams. On shutdown
/// the running service is cancelled and then awaited, so its cleanup - closing
/// the transport - has finished before this returns.
///
/// # Errors
/// The same as [`serve_stdio`].
async fn serve_stdio_on<R, W>(
    server: PromptForgeServer,
    read: R,
    write: W,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), ServeError>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let transport = stdio::BoundedStdioTransport::new(read, write);
    let mut shutdown = std::pin::pin!(shutdown);
    // The handshake is part of what shutdown must be able to interrupt: a peer
    // that connects and never sends `initialize` would otherwise park the
    // session here forever. Racing the serve future against the signal lets a
    // shutdown drop the half-open transport and return rather than hang.
    let serve = server.serve(transport);
    let running = {
        let mut serve = std::pin::pin!(serve);
        tokio::select! {
            result = serve.as_mut() => {
                result.map_err(ServeError::stdio)?
            }
            () = shutdown.as_mut() => return Ok(()),
        }
    };
    // Taken before `waiting` moves the service: a clone of its cancellation
    // token, so the shutdown branch can stop the session the waiter is parked
    // on and then await the same waiter for the clean close.
    let cancel = running.cancellation_token();
    let mut waiting = std::pin::pin!(running.waiting());
    tokio::select! {
        result = waiting.as_mut() => {
            result.map_err(ServeError::stdio)?;
        }
        () = shutdown.as_mut() => {
            cancel.cancel();
            waiting
                .as_mut()
                .await
                .map_err(ServeError::stdio)?;
        }
    }
    Ok(())
}

/// The liveness probe: unauthenticated, and 200 for as long as the process
/// serves.
async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "serving" }))
}

/// Checks the shared bearer on one HTTP request.
///
/// Per request rather than per session, so a caller whose presented token no
/// longer matches is refused on the very next call on a session that already
/// initialized. A refusal carries `WWW-Authenticate: Bearer`, which is what
/// tells a client the scheme it failed rather than that the endpoint is gone.
async fn require_bearer(
    State(token): State<Arc<Secret>>,
    request: Request,
    next: Next,
) -> Response {
    // A missing header, one whose scheme is not `Bearer`, or one that presents
    // no credential at all is refused here rather than compared. Falling through
    // with the empty string is what let an empty configured token authenticate a
    // caller presenting nothing, and the configuration refuses an empty token as
    // well, so neither a typo nor a comparison bug can open the surface alone.
    let presented = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_credential);
    let Some(presented) = presented.filter(|credential| !credential.is_empty()) else {
        return unauthorized();
    };
    if constant_time_eq(presented.as_bytes(), token.expose().as_bytes()) {
        next.run(request).await
    } else {
        unauthorized()
    }
}

/// The credential from an `Authorization: Bearer <credential>` header.
///
/// The scheme is matched case-insensitively, which RFC 7235 requires: a client
/// that sends `bearer` rather than `Bearer` is presenting the same scheme. The
/// credential is returned as it stands, empty included, because whether an empty
/// credential authenticates is the caller's decision and not this parser's.
fn bearer_credential(header: &str) -> Option<&str> {
    let (scheme, credential) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("Bearer")
        .then_some(credential.trim())
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
