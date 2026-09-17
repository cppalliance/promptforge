//! The queue cancellation routes: `POST /admin/queue/cancel` and
//! `POST /admin/queue/cancel-pending`.

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use crate::AppState;
use crate::auth::{Caller, check_auth};
use crate::error::GatewayError;

/// The `POST /admin/queue/cancel` route: bearer-authed, fires the active
/// command's cancellation token. The reply reports whether a command was
/// active to cancel; the command settles as cancelled at its next chunk
/// or phase boundary.
pub(crate) async fn admin_queue_cancel(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let cancelled = state.commands.cancel_active();
    Ok(Json(serde_json::json!({ "cancelled": cancelled })))
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
    caller: Caller,
    Json(request): Json<CancelPendingRequest>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let cancelled = state.commands.cancel_pending(request.index);
    Ok(Json(serde_json::json!({ "cancelled": cancelled })))
}
