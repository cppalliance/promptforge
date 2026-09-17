//! The `GET /admin/config` route: the running global configuration as
//! JSON with secrets redacted.

use axum::Json;
use axum::extract::State;

use crate::AppState;
use crate::auth::{Caller, check_auth};
use crate::error::GatewayError;

/// The `GET /admin/config` route: bearer-authed, renders the running global
/// config in the pending admin shape. The running profile is not part of
/// the document (`GET /admin/status` reports it), so the reply round-trips
/// through `PUT /admin/config` unchanged.
pub(crate) async fn admin_config(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let live = state.live.read().await;
    Ok(Json(live.config.to_json()))
}
