//! The `POST /admin/restart` route: restarts the gateway under its supervisor.
//!
//! It fires the shutdown signal with [`ShutdownReason::Restart`]. The process
//! exits with [`RESTART_REQUESTED`](crate::RESTART_REQUESTED) and the supervisor
//! starts a fresh gateway. Like `POST /shutdown`, it answers `202 Accepted`
//! before the graceful drain closes its connection.

use axum::Router;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::routing::post;

use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::GatewayError;
use crate::registry::RouteInfo;
use crate::shutdown::ShutdownReason;

const RESTART: RouteInfo = RouteInfo::walled("/admin/restart", &[Method::POST]);

/// The restart route, as the registry sees it.
pub(super) const ROUTES: &[RouteInfo] = &[RESTART];

/// The restart route.
pub(super) fn routes() -> Router<AppState> {
    Router::new().route(RESTART.path, post(admin_restart))
}

/// The `POST /admin/restart` route: bearer-authed, restarts the gateway
/// under its supervisor.
///
/// Refuses with [`GatewayError::RestartUnavailable`] (409) when no
/// supervisor runs this gateway. Applies nothing: pending changes stay
/// staged, and applied ones take effect at boot.
async fn admin_restart(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<StatusCode, GatewayError> {
    if !state.supervised {
        return Err(GatewayError::RestartUnavailable);
    }

    // As for `POST /shutdown`: cancel the active queue command so the
    // drain, and the exit, stay prompt.
    state.commands.cancel_active();
    state.shutdown.fire(ShutdownReason::Restart);

    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
#[path = "restart-tests.rs"]
mod tests;
