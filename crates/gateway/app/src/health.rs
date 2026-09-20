//! The `GET /health` liveness probe.

use axum::http::Method;
use axum::routing::get;
use axum::{Json, Router};

use crate::AppState;
use crate::registry::RouteInfo;

const HEALTH: RouteInfo = RouteInfo::open("/health", &[Method::GET]);

/// The health route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[HEALTH];

/// The health route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(HEALTH.path, get(health))
}

/// Liveness probe; unauthenticated and always 200 while serving.
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "serving" }))
}
