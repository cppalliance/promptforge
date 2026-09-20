//! The `POST /shutdown` route, the remote face of
//! [`crate::shutdown::ShutdownSignal`].
//!
//! The route is the remote face of the graceful shutdown that Ctrl-C and
//! [`GatewayHandle::shutdown`](crate::GatewayHandle::shutdown) drive; the
//! tray's Quit and the shell's Quit-everything call it. It sits behind the
//! shared loopback wall and bearer auth, and it answers `202 Accepted`
//! while its own request is still in flight: axum's graceful shutdown
//! drains in-flight requests before closing their connections, so the
//! response always reaches the caller ahead of the shutdown it asked for.

use axum::Router;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::routing::post;

use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::GatewayError;
use crate::registry::RouteInfo;

const SHUTDOWN: RouteInfo = RouteInfo::walled("/shutdown", &[Method::POST]);

/// The shutdown route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[SHUTDOWN];

/// The shutdown route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(SHUTDOWN.path, post(admin_shutdown))
}

/// The `POST /shutdown` route: bearer-authed, loopback-only via the shared
/// wall, answering `202 Accepted` and firing the shutdown signal.
///
/// Like every bearer route it inherits the configured key, including the
/// deliberately credential-free empty-key configuration.
pub(crate) async fn admin_shutdown(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<StatusCode, GatewayError> {
    // Cancel the active queue command first: a shutdown during provisioning
    // stops the download, so the serve loop's drain and the process exit
    // stay prompt.
    state.commands.cancel_active();
    state.shutdown.fire();
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
#[path = "shutdown-tests.rs"]
mod tests;
