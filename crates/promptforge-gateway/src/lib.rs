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
//! inference via a managed `llama-server` subprocess (`[[local_model]]`), named
//! profiles with recursive `include` and immediate `POST /admin/switch-profile`,
//! a bearer-authed `GET /v1/models` catalog, a Brave-backed
//! `POST /v1/tools/web_search` configured by `[tools.web_search]`, and
//! `GET /health`. In-process llama.cpp FFI, endpoint pinning, model packs,
//! streaming, and the Anthropic protocol shim are deferred.

mod api_error;
mod config;
mod error;
mod http_util;
mod local;
mod profile;
mod queue;
mod routing;
mod runner;
mod tools;
mod upstream;
mod web_search_process;
mod wire;

pub use crate::api_error::{
    ConfigError, ConfigErrorKind, ServeError, StartupError, StartupErrorKind,
};
pub use crate::config::{Config, Secret};
pub use crate::profile::{ProfileName, ProfileNameError, default_profiles_dir};
pub use crate::runner::{ConfigSource, Gateway, ProfilesContext, ServeOptions, run};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::routing::{get, post};
use axum::{Router, response::IntoResponse};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::config::WebSearchConfig;
use crate::error::GatewayError;
use crate::local::LocalRuntime;
use crate::routing::Routing;
use crate::tools::WebSearchState;
use crate::wire::{ChatRequest, ChatResponse, ModelInfo, ModelsResponse};

/// Mutable live configuration held behind a lock so profile switches can swap
/// routing and local children without rebuilding the axum router.
#[derive(Debug)]
struct LiveState {
    routing: Arc<Routing>,
    key: Secret,
    web_search: Option<Arc<WebSearchState>>,
    local: LocalRuntime,
    profile_name: Option<String>,
}

/// Directory used by admin profile routes.
#[derive(Debug)]
struct AdminProfiles {
    dir: PathBuf,
}

/// Shared handler state: live routing/key/local runtime, and optional profiles dir.
#[derive(Debug, Clone)]
pub(crate) struct AppState {
    live: Arc<RwLock<LiveState>>,
    profiles: Option<Arc<AdminProfiles>>,
    /// Serializes profile switches so two concurrent switches cannot interleave
    /// their reads and writes of the live state.
    switch: Arc<tokio::sync::Mutex<()>>,
}

impl AppState {
    /// Build full runtime state for `Gateway` and integration tests.
    #[must_use]
    pub(crate) fn from_parts(
        routing: Arc<Routing>,
        key: Secret,
        local: LocalRuntime,
        web_search: Option<&WebSearchConfig>,
        profiles_dir: Option<PathBuf>,
        profile_name: Option<String>,
    ) -> AppState {
        AppState {
            live: Arc::new(RwLock::new(LiveState {
                routing,
                key,
                web_search: web_search.map(|cfg| Arc::new(WebSearchState::new(cfg))),
                local,
                profile_name,
            })),
            profiles: profiles_dir.map(|dir| Arc::new(AdminProfiles { dir })),
            switch: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// The web-search capability, when configured.
    pub(crate) async fn web_search(&self) -> Option<Arc<WebSearchState>> {
        self.live.read().await.web_search.clone()
    }
}

/// Build the gateway's axum router.
pub(crate) fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/tools/web_search", post(tools::web_search))
        .route("/health", get(health))
        .route("/admin/profiles", get(admin_list_profiles))
        .route("/admin/status", get(admin_status))
        .route("/admin/switch-profile", post(admin_switch_profile))
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
    check_auth(&state, &headers).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = {
        let live = state.live.read().await;
        live.routing.model(&request.model)?
    };
    let client_id = crate::queue::ClientId::from_header(
        headers
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let _permit = model.endpoint.lane.admit(client_id.as_str()).await?;
    let response = model
        .endpoint
        .upstream
        .send(request, &model.upstream_name)
        .await?;
    response
        .validate()
        .map_err(|reason| GatewayError::UpstreamStatus {
            status: 502,
            body: reason.to_owned(),
        })?;
    Ok(Json(response))
}

/// Bearer-authed catalog of configured models for host bind.
async fn list_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ModelsResponse>, GatewayError> {
    check_auth(&state, &headers).await?;
    let live = state.live.read().await;
    let data = live
        .routing
        .models()
        .iter()
        .map(|model| ModelInfo {
            id: model.name.clone(),
            object: "model",
            description: model.description.clone(),
            context: model.context,
            thinking: model.thinking,
            tool_dialect: model.tool_dialect.clone(),
            tools_mode: model.tools_mode.clone(),
        })
        .collect();
    Ok(Json(ModelsResponse {
        object: "list",
        data,
    }))
}

#[derive(Debug, Deserialize)]
struct SwitchProfileRequest {
    name: String,
}

/// Lists `*.toml` stems in the profiles directory.
async fn admin_list_profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let dir = profiles_dir(&state)?;
    let profiles =
        profile::list_profiles(dir).map_err(|e| GatewayError::SwitchFailed(e.to_string()))?;
    Ok(Json(serde_json::json!({ "profiles": profiles })))
}

/// Current profile name, loaded model names, and a queue note.
async fn admin_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let live = state.live.read().await;
    let models: Vec<&str> = live
        .routing
        .models()
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    Ok(Json(serde_json::json!({
        "profile": live.profile_name,
        "models": models,
        "local_children": live.local.child_count(),
        "queue": "per-endpoint waiting queue; switch-profile is immediate (no drain)",
    })))
}

/// Immediately switches to another named profile.
///
/// Switches are serialized by a dedicated mutex, so two concurrent requests
/// cannot interleave. Configuration is loaded and validated off the live lock;
/// a config or routing failure returns an error and leaves the live state
/// untouched. The bearer key, routing, and web-search settings of the new
/// profile are committed only after the new local runtime starts successfully,
/// via a single atomic swap under the write lock, so a failed switch never
/// rotates the admin credential. Because old and new `llama-server` children
/// must not both hold VRAM, the old children are stopped before the new ones
/// start; a start failure therefore leaves the previous profile authenticated
/// and remote-routable but without its local models (a documented degraded
/// state) rather than a half-applied new profile.
async fn admin_switch_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SwitchProfileRequest>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let dir = profiles_dir(&state)?.to_path_buf();
    let name = crate::profile::ProfileName::parse(&request.name)
        .map_err(|e| GatewayError::SwitchFailed(e.to_string()))?;

    // Serialize switches for the whole operation (LIB-008).
    let _switch = state.switch.lock().await;

    let path = dir.join(format!("{name}.toml"));
    if !path.is_file() {
        return Err(GatewayError::ProfileNotFound(name.to_string()));
    }

    // Build and validate the entire remote side off the live lock. Any failure
    // here returns before mutating live state at all (LIB-009).
    let config = crate::config::Config::load_profile(&dir, &name)
        .map_err(|e| GatewayError::SwitchFailed(e.to_string()))?;
    let remote_routing =
        Routing::from_config(&config).map_err(|e| GatewayError::SwitchFailed(e.to_string()))?;
    let new_web_search = config
        .web_search_config()
        .map(WebSearchState::new)
        .map(Arc::new);
    let new_key = config.server_key();

    // Stop the previous local children before starting new ones so the two
    // never hold VRAM simultaneously. The bearer key, routing, and web-search
    // settings are left untouched here, so auth stays stable if start fails.
    let old_local = {
        let mut live = state.live.write().await;
        std::mem::replace(&mut live.local, LocalRuntime::empty())
    };
    drop(old_local);

    let new_local = match tokio::task::spawn_blocking(move || LocalRuntime::start(&config)).await {
        Ok(Ok(runtime)) => runtime,
        Ok(Err(e)) => {
            return Err(GatewayError::SwitchFailed(e.to_string()));
        }
        Err(e) => {
            return Err(GatewayError::SwitchFailed(format!(
                "local runtime start task failed: {e}"
            )));
        }
    };

    let routing = remote_routing
        .merge(new_local.models().iter().cloned())
        .map_err(|e| GatewayError::SwitchFailed(e.to_string()))?;

    // Atomic swap: commit the whole new profile at once.
    {
        let mut live = state.live.write().await;
        live.routing = Arc::new(routing);
        live.key = new_key;
        live.web_search = new_web_search;
        live.local = new_local;
        live.profile_name = Some(name.to_string());
    }

    tracing::info!(profile = %name, "switched profile");
    Ok(Json(serde_json::json!({
        "ok": true,
        "profile": name.to_string(),
    })))
}

fn profiles_dir(state: &AppState) -> Result<&Path, GatewayError> {
    state
        .profiles
        .as_ref()
        .map(|ctx| ctx.dir.as_path())
        .ok_or(GatewayError::ProfilesUnavailable)
}

/// Compare the request's bearer token against the configured token.
pub(crate) async fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), GatewayError> {
    let presented = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    let live = state.live.read().await;
    if secret_eq(presented.as_bytes(), live.key.expose().as_bytes()) {
        Ok(())
    } else {
        Err(GatewayError::Unauthorized)
    }
}

/// Constant-time credential comparison.
///
/// Both inputs are hashed to fixed-length SHA-256 digests before comparison, so
/// the comparison operates on equal-length data (no early length-based
/// short-circuit) and leaks neither the configured key's length nor its bytes.
/// The digest comparison uses the `subtle` crate's constant-time primitive.
fn secret_eq(presented: &[u8], configured: &[u8]) -> bool {
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;

    let presented = Sha256::digest(presented);
    let configured = Sha256::digest(configured);
    presented.ct_eq(&configured).into()
}

#[cfg(test)]
mod auth_tests {
    use super::secret_eq;

    #[test]
    fn equal_secrets_match() {
        assert!(secret_eq(b"s3cret-token", b"s3cret-token"));
    }

    #[test]
    fn unequal_secrets_do_not_match() {
        assert!(!secret_eq(b"s3cret-token", b"wrong-token"));
        assert!(!secret_eq(b"", b"nonempty"));
        assert!(!secret_eq(b"short", b"a-much-longer-token"));
    }

    #[test]
    fn empty_matches_empty() {
        assert!(secret_eq(b"", b""));
    }
}
