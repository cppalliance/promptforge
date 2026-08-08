//! PromptForge inference gateway.
//!
//! A small always-on service that accepts OpenAI-shaped chat completions, holds
//! the backend credential, resolves the request's model name to a configured
//! endpoint, forwards the request, and relays the reply. It is the only process
//! in the system with an edge to an LLM backend, so the executor above it never
//! holds a vendor key.
//!
//! What ships: one OpenAI passthrough at `POST /v1/chat/completions` with
//! bearer auth and model routing, per-endpoint concurrency limits with a fair
//! waiting queue (`[queue]` / `concurrency`), gateway-owned local generative
//! inference via a managed `llama-server` subprocess (`[[local_model]]`), a
//! bearer-authed `GET /v1/models` catalog, a Brave-backed
//! `POST /v1/tools/web_search` configured by `[tools.web_search]`, and
//! `GET /health`. In-process llama.cpp FFI, profiles, endpoint pinning, model
//! packs, hot reload, streaming, and the Anthropic protocol shim are deferred.

pub mod config;
pub mod error;
pub mod local;
pub mod queue;
pub mod routing;
pub mod tools;
pub mod upstream;
pub mod wire;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::routing::{get, post};
use axum::{Router, response::IntoResponse};

use crate::config::{Secret, WebSearchConfig};
use crate::error::GatewayError;
use crate::routing::Routing;
use crate::tools::WebSearchState;
use crate::wire::{ChatRequest, ChatResponse, ModelInfo, ModelsResponse};

/// Shared handler state: the routing table, the shared bearer token, and the
/// optional web-search capability.
#[derive(Debug, Clone)]
pub struct AppState {
    routing: Arc<Routing>,
    token: Arc<Secret>,
    web_search: Option<Arc<WebSearchState>>,
}

impl AppState {
    /// Build handler state from a routing table and the server token. The
    /// web-search capability is absent; add it with [`AppState::with_web_search`].
    #[must_use]
    pub fn new(routing: Arc<Routing>, token: Secret) -> AppState {
        AppState {
            routing,
            token: Arc::new(token),
            web_search: None,
        }
    }

    /// Enable the web-search tool from its configuration.
    #[must_use]
    pub fn with_web_search(mut self, cfg: &WebSearchConfig) -> AppState {
        self.web_search = Some(Arc::new(WebSearchState::new(cfg)));
        self
    }

    /// The web-search capability, when configured.
    #[must_use]
    pub(crate) fn web_search(&self) -> Option<&WebSearchState> {
        self.web_search.as_deref()
    }
}

/// Build the gateway's axum router.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/tools/web_search", post(tools::web_search))
        .route("/health", get(health))
        .with_state(state)
}

/// Liveness probe; unauthenticated and always 200 while serving.
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "serving" }))
}

/// Header naming the caller for fair queue scheduling. Absent → `"default"`.
const CLIENT_HEADER: &str = "X-PromptForge-Client";

/// The one route that reaches a backend.
async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, GatewayError> {
    check_auth(&state, &headers)?;
    let model = state.routing.model(&request.model)?;
    let client_key = headers
        .get(CLIENT_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("default");
    let _permit = model.endpoint.lane.admit(client_key).await?;
    let response = model
        .endpoint
        .upstream
        .send(request, &model.upstream_name)
        .await?;
    Ok(Json(response))
}

/// Bearer-authed catalog of configured models for host bind.
async fn list_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ModelsResponse>, GatewayError> {
    check_auth(&state, &headers)?;
    let data = state
        .routing
        .models()
        .iter()
        .map(|model| ModelInfo {
            id: model.name.clone(),
            object: "model",
            description: model.description.clone(),
            context: model.context,
            thinking: model.thinking,
        })
        .collect();
    Ok(Json(ModelsResponse {
        object: "list",
        data,
    }))
}

/// Compare the request's bearer token against the configured token.
pub(crate) fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), GatewayError> {
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
