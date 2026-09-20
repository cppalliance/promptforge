//! The `POST /shutdown` route and the process-shutdown signal it fires.
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
use tokio_util::sync::CancellationToken;

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

/// The process-shutdown signal shared by the `POST /shutdown` route, the
/// serve loop (which selects on it alongside the caller-owned shutdown
/// future), and every open-ended response stream, which ends when it fires
/// so the graceful drain has nothing left to wait for.
///
/// Every clone shares the one underlying signal. It is a cancellation
/// token, not a notify: a `fire` wakes every waiter at once and stays
/// fired, so a stream that subscribes after the signal ends immediately.
#[derive(Debug, Clone, Default)]
pub(crate) struct ShutdownSignal {
    token: CancellationToken,
}

impl ShutdownSignal {
    /// Fires the signal, starting the serve loop's graceful shutdown.
    pub(crate) fn fire(&self) {
        self.token.cancel();
    }

    /// Whether the signal has been fired; the tray's status tick reads it
    /// to tell a requested shutdown apart from a serve-loop failure.
    pub(crate) fn is_fired(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Resolves once the signal has fired.
    pub(crate) async fn fired(&self) {
        self.token.cancelled().await;
    }
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
