//! PromptForge inference gateway.
//!
//! A small always-on service that accepts OpenAI-shaped chat completions, holds
//! the backend credential, resolves the request's model name to a configured
//! endpoint, forwards the request, and relays the reply. It is the only process
//! in the system with an edge to an LLM backend, so the executor above it never
//! holds a vendor key.
//!
//! v0 is a walking skeleton: one OpenAI passthrough endpoint, bearer auth, model
//! routing, `POST /v1/chat/completions`, and `GET /health`. Admission control,
//! endpoint pinning, model packs, hot reload, streaming, and the Anthropic
//! protocol shim are deferred.

pub mod config;
pub mod error;
pub mod routing;
pub mod upstream;
pub mod wire;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::routing::{get, post};
use axum::{Router, response::IntoResponse};

use crate::config::Secret;
use crate::error::GatewayError;
use crate::routing::Routing;
use crate::wire::{ChatRequest, ChatResponse};

/// Shared handler state: the routing table and the shared bearer token.
#[derive(Clone)]
pub struct AppState {
    routing: Arc<Routing>,
    token: Arc<Secret>,
}

impl AppState {
    /// Build handler state from a routing table and the server token.
    #[must_use]
    pub fn new(routing: Arc<Routing>, token: Secret) -> AppState {
        AppState {
            routing,
            token: Arc::new(token),
        }
    }
}

/// Build the gateway's axum router.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/health", get(health))
        .with_state(state)
}

/// Liveness probe; unauthenticated and always 200 while serving.
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "serving" }))
}

/// The one route that reaches a backend.
async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, GatewayError> {
    check_auth(&state, &headers)?;
    let model = state.routing.model(&request.model)?;
    let response = model
        .endpoint
        .upstream
        .send(request, &model.upstream_name)
        .await?;
    Ok(Json(response))
}

/// Compare the request's bearer token against the configured token.
fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), GatewayError> {
    let presented = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if constant_time_eq(presented.as_bytes(), state.token.expose().as_bytes()) {
        Ok(())
    } else {
        Err(GatewayError::Unauthorized)
    }
}

/// Length-checked constant-time byte comparison.
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
