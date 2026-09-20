//! The queue cancellation routes: `POST /admin/queue/cancel` and
//! `POST /admin/queue/cancel-pending`.

use axum::extract::State;
use axum::http::Method;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::{GatewayError, WireJson};
use crate::registry::RouteInfo;

/// The reply of both cancellation routes.
#[derive(Debug, Serialize)]
pub(crate) struct CancelReply {
    /// Whether there was a command to cancel.
    cancelled: bool,
}

const CANCEL: RouteInfo = RouteInfo::open("/admin/queue/cancel", &[Method::POST]);
const CANCEL_PENDING: RouteInfo = RouteInfo::open("/admin/queue/cancel-pending", &[Method::POST]);

/// The queue cancellation routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[CANCEL, CANCEL_PENDING];

/// The queue cancellation routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(CANCEL.path, post(admin_queue_cancel))
        .route(CANCEL_PENDING.path, post(admin_queue_cancel_pending))
}

/// The `POST /admin/queue/cancel` route: bearer-authed, fires the active
/// command's cancellation token. The reply reports whether a command was
/// active to cancel; the command settles as cancelled at its next chunk
/// or phase boundary.
pub(crate) async fn admin_queue_cancel(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<CancelReply>, GatewayError> {
    let cancelled = state.commands.cancel_active();
    Ok(Json(CancelReply { cancelled }))
}

/// The `POST /admin/queue/cancel-pending` request body.
#[derive(Debug, Deserialize)]
pub(crate) struct CancelPendingRequest {
    index: usize,
}

/// The `POST /admin/queue/cancel-pending` route: bearer-authed, removes
/// the waiting command at `index`, settling its waiters as cancelled. The
/// reply reports whether an entry was removed.
pub(crate) async fn admin_queue_cancel_pending(
    State(state): State<AppState>,
    _caller: AuthedCaller,
    WireJson(request): WireJson<CancelPendingRequest>,
) -> Result<Json<CancelReply>, GatewayError> {
    let cancelled = state.commands.cancel_pending(request.index);
    Ok(Json(CancelReply { cancelled }))
}
