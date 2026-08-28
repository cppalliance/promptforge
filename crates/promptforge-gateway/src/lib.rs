//! PromptForge inference gateway.
//!
//! A small always-on service that accepts OpenAI-shaped chat completions, holds
//! the backend credential, resolves the request's model name to a configured
//! endpoint, forwards the request, and relays the reply. It is the only process
//! in the system with an edge to an LLM backend, so the executor above it never
//! holds a vendor key.
//!
//! What ships: one OpenAI passthrough at `POST /v1/chat/completions` with
//! bearer auth, model routing, and a typed SSE relay for `stream: true`, an
//! embeddings passthrough at
//! `POST /v1/embeddings` for `kind = "embedding"` models, a rerank
//! passthrough at `POST /v1/rerank` for `kind = "classifier"` models, shared
//! concurrency pools with bounded, fair waiting queues (`[[dominion]]`),
//! gateway-owned local generative inference via a managed `llama-server`
//! subprocess (`[[local_model]]`), named profiles with recursive `include`
//! and immediate `POST /admin/switch-profile` streaming its stages over
//! SSE, a bearer-authed
//! `GET /v1/models` catalog, a Brave-backed `POST /v1/tools/web_search`
//! configured by `[tools.web_search]`, an on-demand blob cache
//! (`POST /v1/cache` with SSE download progress, `GET /v1/cache`,
//! `DELETE /v1/cache/{sha256}`) backed by the local artifact store, and
//! `GET /health`. In-process
//! llama.cpp FFI and endpoint pinning are deferred.

mod api_error;
mod cache;
mod dialect;
mod error;
mod http_util;
mod local;
mod queue;
mod routing;
mod runner;
#[cfg(test)]
mod testsupport;
mod tools;
mod upstream;
mod web_search_process;
mod wire;

pub use crate::api_error::{ServeError, StartupError, StartupErrorKind};
pub use crate::runner::{Gateway, GatewayHandle, ProfilesContext, ServeOptions, run, spawn};
pub use promptforge_gateway_config::{
    Config, ConfigError, ConfigErrorKind, ProfileName, ProfileNameError, Secret,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Router, response::IntoResponse};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::GatewayError;
use crate::local::LocalRuntime;
use crate::routing::Routing;
use crate::tools::WebSearchState;
use crate::wire::{
    ChatRequest, EmbeddingRequest, EmbeddingResponse, ModelInfo, ModelsResponse, RerankRequest,
    RerankResponse,
};
use promptforge_gateway_config::{ModelKind, ServerConfig, WebSearchConfig, WorkshopConfig};

/// Mutable live configuration held behind a lock so profile switches can swap
/// routing and local children without rebuilding the axum router.
#[derive(Debug)]
struct LiveState {
    routing: Arc<Routing>,
    key: Secret,
    web_search: Option<Arc<WebSearchState>>,
    local: LocalRuntime,
    profile_name: Option<String>,
    /// The active profile's `models` allowlist, when it declared one.
    model_allowlist: Option<Vec<String>>,
}

/// Directory used by admin profile routes.
#[derive(Debug)]
struct AdminProfiles {
    dir: PathBuf,
}

/// What the active profile selected: its name and its `models` allowlist.
/// Both are reported by `GET /admin/status` and swapped together on a
/// profile switch.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProfileSelection {
    /// The active profile name.
    pub(crate) name: Option<String>,
    /// The active profile's `models` allowlist, when it declared one.
    pub(crate) model_allowlist: Option<Vec<String>>,
}

/// The boot file's boot-owned sections, fixed for the process lifetime and
/// enforced against every profile switch.
#[derive(Debug, Clone)]
pub(crate) struct BootOwned {
    /// The boot `[server]`: the socket and the gateway bearer key.
    pub(crate) server: ServerConfig,
    /// The boot `[workshop]`, when present: the hosted workshop is started
    /// once at boot and cannot be moved, reconfigured, or removed mid-run.
    pub(crate) workshop: Option<WorkshopConfig>,
}

/// Shared handler state: live routing/key/local runtime, the boot-owned
/// `[server]` and `[workshop]` settings, and optional profiles dir.
#[derive(Debug, Clone)]
pub(crate) struct AppState {
    live: Arc<RwLock<LiveState>>,
    profiles: Option<Arc<AdminProfiles>>,
    /// The boot-owned sections, retained so profile switches can refuse a
    /// profile that changes them.
    boot: Arc<BootOwned>,
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
        selection: ProfileSelection,
        boot: BootOwned,
    ) -> AppState {
        AppState {
            live: Arc::new(RwLock::new(LiveState {
                routing,
                key,
                web_search: web_search.map(|cfg| Arc::new(WebSearchState::new(cfg))),
                local,
                profile_name: selection.name,
                model_allowlist: selection.model_allowlist,
            })),
            profiles: profiles_dir.map(|dir| Arc::new(AdminProfiles { dir })),
            boot: Arc::new(boot),
            switch: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// The web-search capability, when configured.
    pub(crate) async fn web_search(&self) -> Option<Arc<WebSearchState>> {
        self.live.read().await.web_search.clone()
    }

    /// The active profile's `[local].cache_dir` setting, for the cache routes.
    pub(crate) async fn cache_dir(&self) -> Option<String> {
        self.live.read().await.local.cache_dir().map(str::to_owned)
    }
}

/// Build the gateway's axum router.
pub(crate) fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/rerank", post(rerank))
        .route("/v1/models", get(list_models))
        .route("/v1/tools/web_search", post(tools::web_search))
        .route("/v1/cache", get(cache::list_cache).post(cache::post_cache))
        .route("/v1/cache/{sha256}", delete(cache::delete_cache))
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

/// The chat route to a backend.
async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatRequest>,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = {
        let live = state.live.read().await;
        live.routing.model(&request.model)?
    };
    crate::routing::require_kind(&model, ModelKind::Chat)?;
    let client_id = crate::queue::ClientId::from_header(
        headers
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let permit = model.endpoint.queue.admit(client_id.as_str()).await?;
    // Emulated dialects rewrite the request (guide injection, tool stripping)
    // and parse the reply's content fences; the emulated parse applies to
    // non-streaming completions only.
    let emulated = model.tool_dialect == crate::dialect::GEMMA3_TOOL_CODE && !request.stream;
    let request = if emulated {
        let mut request = request;
        crate::dialect::prepare_request(&mut request)?;
        request
    } else {
        request
    };
    if request.stream {
        // A failure here is before the SSE response starts, so it is
        // consumed as a normal JSON error, never a stream that dies
        // mid-flight.
        let streamed = model
            .endpoint
            .upstream
            .stream(request, &model.upstream_name)
            .await?;
        return Ok(relay_sse(streamed, permit));
    }
    let response = model
        .endpoint
        .upstream
        .send(request, &model.upstream_name)
        .await?;
    response
        .validate()
        .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
    let mut response = response;
    if emulated {
        crate::dialect::apply_response(&mut response, &model.name);
    }
    Ok(Json(response).into_response())
}

/// Re-emit a validated upstream chunk stream as an SSE response, holding the
/// dominion queue permit for the stream's lifetime.
///
/// The relay is typed: each upstream chunk is validated and re-serialized per
/// chunk rather than splicing upstream bytes through. A mid-stream failure is
/// emitted as an error-envelope `data:` event before the stream ends, and a
/// clean end is marked with the `data: [DONE]` sentinel. The response
/// forwards the upstream `Content-Type`/`Cache-Control` when present,
/// defaulting to `text/event-stream`/`no-cache`.
///
/// Client-disconnect cancellation is Drop all the way down: when the client
/// goes away the response body is dropped, which drops the chunk stream,
/// which drops the upstream response and aborts the upstream connection,
/// releasing the permit in the same unwind. There is no explicit cancel path.
fn relay_sse(streamed: crate::upstream::StreamedChunks, permit: crate::queue::Permit) -> Response {
    use futures_util::StreamExt as _;

    let relayed = streamed
        .chunks
        .scan((false, permit), |(failed, _permit), item| {
            if *failed {
                return std::future::ready(None);
            }
            let line = match item {
                Ok(chunk) => match serde_json::to_string(&chunk) {
                    Ok(json) => format!("data: {json}\n\n"),
                    Err(error) => {
                        *failed = true;
                        format!(
                            "data: {}\n\n",
                            GatewayError::upstream_protocol(error).envelope()
                        )
                    }
                },
                Err(error) => {
                    *failed = true;
                    format!("data: {}\n\n", error.envelope())
                }
            };
            std::future::ready(Some(Ok::<String, std::convert::Infallible>(line)))
        });
    let done = futures_util::stream::once(async { Ok("data: [DONE]\n\n".to_owned()) });
    let mut response = Response::new(Body::from_stream(relayed.chain(done)));
    let headers = response.headers_mut();
    let content_type = streamed
        .content_type
        .and_then(|value| HeaderValue::from_str(&value).ok())
        .unwrap_or_else(|| HeaderValue::from_static("text/event-stream"));
    headers.insert(CONTENT_TYPE, content_type);
    let cache_control = streamed
        .cache_control
        .and_then(|value| HeaderValue::from_str(&value).ok())
        .unwrap_or_else(|| HeaderValue::from_static("no-cache"));
    headers.insert(CACHE_CONTROL, cache_control);
    response
}

/// The embeddings route to a backend: the same auth, routing, kind guard, and
/// dominion queue admission as chat, for `kind = "embedding"` models.
async fn embeddings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<EmbeddingRequest>,
) -> Result<Json<EmbeddingResponse>, GatewayError> {
    check_auth(&state, &headers).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = {
        let live = state.live.read().await;
        live.routing.model(&request.model)?
    };
    crate::routing::require_kind(&model, ModelKind::Embedding)?;
    let client_id = crate::queue::ClientId::from_header(
        headers
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let _permit = model.endpoint.queue.admit(client_id.as_str()).await?;
    let response = model
        .endpoint
        .upstream
        .send_embeddings(request, &model.upstream_name)
        .await?;
    response
        .validate()
        .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
    Ok(Json(response))
}

/// The rerank route to a backend: the same auth, routing, kind guard, and
/// dominion queue admission as chat, for `kind = "classifier"` models.
async fn rerank(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RerankRequest>,
) -> Result<Json<RerankResponse>, GatewayError> {
    check_auth(&state, &headers).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = {
        let live = state.live.read().await;
        live.routing.model(&request.model)?
    };
    crate::routing::require_kind(&model, ModelKind::Classifier)?;
    let client_id = crate::queue::ClientId::from_header(
        headers
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let _permit = model.endpoint.queue.admit(client_id.as_str()).await?;
    let response = model
        .endpoint
        .upstream
        .send_rerank(request, &model.upstream_name)
        .await?;
    response
        .validate()
        .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
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
            kind: model.kind,
            description: model.description.clone(),
            context: model.context,
            thinking: model.thinking,
            capabilities: model.capabilities.clone(),
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
    let profiles = promptforge_gateway_config::list_profiles(dir)
        .map_err(|e| GatewayError::switch_failed("list-profiles", e))?;
    Ok(Json(serde_json::json!({ "profiles": profiles })))
}

/// Current profile name, loaded model names, the profile's model allowlist,
/// and a queue note.
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
        "model_allowlist": live.model_allowlist,
        "local_children": live.local.child_count(),
        "queue": "per-dominion shared waiting queue; switch-profile is immediate (no drain)",
    })))
}

/// Immediately switches to another named profile, streaming its progress.
///
/// The reply is `text/event-stream`: a `{"stage": ...}` event opens each
/// phase in execution order - `loading-profile` around config load and
/// validation, `stopping-models` before the old local children shut down,
/// `starting-models` before the new children load their weights into VRAM
/// (the long pole) - and the stream ends with exactly one terminal event,
/// `{"status": "ready", "profile": ...}` or `{"status": "error",
/// "message": ...}`. There is no drain stage because the gateway does not
/// drain. A refusal before the switch starts (bad auth, no profiles
/// directory, a malformed name) stays a buffered JSON error envelope.
///
/// The switch itself runs on its own task ([`run_switch`]) and always runs
/// to completion: a client disconnect drops only the response body and the
/// stage receiver, never the half-finished switch.
async fn admin_switch_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SwitchProfileRequest>,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    let dir = profiles_dir(&state)?.to_path_buf();
    let name = ProfileName::parse(&request.name)
        .map_err(|e| GatewayError::switch_failed("parse-name", e))?;
    // Three stage markers into a bound of eight: try_send never drops here,
    // and even a dropped marker would cost a progress line, not the switch.
    let (stages, rx) = tokio::sync::mpsc::channel(8);
    let switch = tokio::spawn(run_switch(state, dir, name, stages));
    Ok(switch_sse_response(rx, switch))
}

/// Executes a profile switch, marking each phase on the `stages` channel.
///
/// Switches are serialized by a dedicated mutex, so two concurrent requests
/// cannot interleave. The new profile's env file loads before the profile
/// itself (the boot file's env file is already in the process environment
/// from startup). Configuration is loaded and validated off the live lock;
/// a config or routing failure returns an error and leaves the live state
/// untouched. The boot file owns `[server]` and `[workshop]`: a profile
/// whose merged `[server]` or `[workshop]` differs from the boot file's is
/// rejected - the refusal reaches the caller as the switch stream's terminal
/// error event - so the socket, the gateway bearer key, and the hosted
/// workshop's settings are fixed for the process lifetime and a switch never
/// rotates the admin credential. The routing and web-search
/// settings of the new profile are committed only after the new local
/// runtime starts successfully, via a single atomic swap under the write
/// lock. Because old and new `llama-server` children must not both hold
/// VRAM, the old children are stopped before the new ones start; a start
/// failure therefore leaves the previous profile authenticated and
/// remote-routable but without its local models (a documented degraded
/// state) rather than a half-applied new profile.
async fn run_switch(
    state: AppState,
    dir: PathBuf,
    name: ProfileName,
    stages: tokio::sync::mpsc::Sender<&'static str>,
) -> Result<String, GatewayError> {
    // Serialize switches for the whole operation (LIB-008).
    let _switch = state.switch.lock().await;

    let _ = stages.try_send("loading-profile");
    let path = dir.join(format!("{name}.toml"));
    if !path.is_file() {
        return Err(GatewayError::ProfileNotFound(name.to_string()));
    }

    // The profile's env file must be in the process environment before the
    // profile is interpolated. dotenvy never overrides, so anything already
    // set (the process environment, the boot env file) wins.
    crate::runner::load_env_file(&path.with_extension("env"));

    // Build and validate the entire remote side off the live lock. Any failure
    // here returns before mutating live state at all (LIB-009).
    let config = Config::load_profile(&dir, &name)
        .map_err(|e| GatewayError::switch_failed("load-profile", e))?;
    crate::runner::check_server_matches_boot(&state.boot.server, config.server(), &name)
        .map_err(|e| GatewayError::switch_failed("server-mismatch", e))?;
    crate::runner::check_workshop_matches_boot(
        state.boot.workshop.as_ref(),
        config.workshop(),
        &name,
    )
    .map_err(|e| GatewayError::switch_failed("workshop-mismatch", e))?;
    let remote_routing = Routing::from_config(&config)
        .map_err(|e| GatewayError::switch_failed("build-routing", e))?;
    let new_web_search = config
        .web_search_config()
        .map(WebSearchState::new)
        .map(Arc::new);
    let new_key = config.server_key();
    let new_allowlist = config.model_allowlist().map(<[String]>::to_vec);

    // Stop the previous local children before starting new ones so the two
    // never hold VRAM simultaneously. The bearer key, routing, and web-search
    // settings are left untouched here, so auth stays stable if start fails.
    let _ = stages.try_send("stopping-models");
    let old_local = {
        let mut live = state.live.write().await;
        std::mem::replace(&mut live.local, LocalRuntime::empty())
    };
    // Explicitly terminate the old children before starting new ones, and abort
    // the switch if teardown fails. Dropping the runtime does not free their
    // VRAM here (the still-live old routing holds Arc<dyn Upstream> clones, so
    // the runtime is not the sole owner - PFGL-MOD-001); the teardown also
    // cancels any in-flight recovery/respawn and disables further respawn, so no
    // old child can outlive the switch (PF-GW-SERVER-004). Every child failure
    // is surfaced, never discarded, so we never start replacements on top of a
    // survivor.
    match tokio::task::spawn_blocking(move || {
        let result = old_local.shutdown();
        drop(old_local);
        result
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(GatewayError::switch_failed("shutdown-local", e)),
        Err(e) => return Err(GatewayError::switch_failed("shutdown-local-task", e)),
    }

    let _ = stages.try_send("starting-models");
    let new_local = match tokio::task::spawn_blocking(move || LocalRuntime::start(&config)).await {
        Ok(Ok(runtime)) => runtime,
        Ok(Err(e)) => {
            return Err(GatewayError::switch_failed("start-local", e));
        }
        Err(e) => {
            return Err(GatewayError::switch_failed("start-local-task", e));
        }
    };

    let routing = remote_routing
        .merge(new_local.models().iter().cloned())
        .map_err(|e| GatewayError::switch_failed("merge-routing", e))?;

    // Atomic swap: commit the whole new profile at once.
    {
        let mut live = state.live.write().await;
        live.routing = Arc::new(routing);
        live.key = new_key;
        live.web_search = new_web_search;
        live.local = new_local;
        live.profile_name = Some(name.to_string());
        live.model_allowlist = new_allowlist;
    }

    tracing::info!(profile = %name, "switched profile");
    Ok(name.to_string())
}

/// Builds the switch-profile SSE response: stage events drained from the
/// channel, then the terminal event from the switch task's join result, so
/// the outcome can never be lost to channel backpressure.
///
/// The channel closes when [`run_switch`] drops its sender, so the stage
/// stream ends before the terminal event is awaited - the same ordering
/// contract as the cache download stream.
fn switch_sse_response(
    mut rx: tokio::sync::mpsc::Receiver<&'static str>,
    switch: tokio::task::JoinHandle<Result<String, GatewayError>>,
) -> Response {
    use futures_util::StreamExt as _;

    let stages = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)).map(|stage| {
        Ok::<_, std::convert::Infallible>(format!(
            "data: {}\n\n",
            serde_json::json!({ "stage": stage })
        ))
    });
    let terminal = futures_util::stream::once(async move {
        let payload = match switch.await {
            Ok(Ok(profile)) => serde_json::json!({ "status": "ready", "profile": profile }),
            Ok(Err(error)) => serde_json::json!({
                "status": "error",
                "message": error_chain(&error),
            }),
            Err(join_error) => serde_json::json!({
                "status": "error",
                "message": format!("switch task failed: {join_error}"),
            }),
        };
        Ok::<_, std::convert::Infallible>(format!("data: {payload}\n\n"))
    });
    let mut response = Response::new(Body::from_stream(stages.chain(terminal)));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// Renders `error` with its full source chain for the terminal SSE error
/// event: the stream has a single `message` field where the JSON envelope
/// had `message` plus `code`, and a bare `switch profile failed at
/// load-profile` without its cause tells the operator nothing.
fn error_chain(error: &GatewayError) -> String {
    use std::fmt::Write as _;

    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        let _ = write!(message, ": {cause}");
        source = cause.source();
    }
    message
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
