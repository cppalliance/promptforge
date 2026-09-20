//! The `GET /health` liveness probe.

use axum::routing::get;
use axum::{Json, Router};

use crate::AppState;

/// The health route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/health", get(health))
}

/// Liveness probe; unauthenticated and always 200 while serving.
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "serving" }))
}
