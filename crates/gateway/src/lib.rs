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
//! subprocess (`[[local_model]]`), named profile checklists from one loaded
//! catalog with bounded-drain `POST /admin/switch-profile` streaming its
//! existing stages over SSE, a bearer-authed
//! `GET /v1/models` catalog, a bearer-authed `GET /admin/config` view of the
//! running global configuration as JSON with secrets redacted, a
//! bearer-authed `GET /admin/progress` SSE
//! stream of the process progress hub, a Brave-backed `POST /v1/tools/web_search`
//! configured by `[tools.web_search]`, an on-demand blob cache
//! (`POST /v1/cache` with SSE download progress, `GET /v1/cache`,
//! `DELETE /v1/cache/{sha256}`) backed by the local artifact store, a
//! bearer-authed `GET /admin/orphans` listing of cache files no loaded
//! `[[local_model]]` entry references (local builds), a bearer-authed
//! `GET /admin/model-info` GGUF-header readout of a cache file's layer and
//! parameter counts (local builds), a bearer-authed
//! `GET /admin/chat-templates` family catalog and per-model effective
//! resolution view (local builds), a bearer-authed
//! `POST /v1/audio/transcriptions` OpenAI-compatible multipart STT endpoint
//! (stt builds), a bearer-authed
//! `GET /admin/system` snapshot of host CPU, RAM, cache-drive, and GPU
//! metrics, a bearer-authed `GET /admin/hf/search` and
//! `GET /admin/hf/model/{repo}` proxy onto the Hugging Face hub API
//! (attaching the process `HF_TOKEN` when set), bearer-authed shadow-file
//! write routes staging pending edits beside the real files without ever
//! touching them (`PUT /admin/config`, `PUT /admin/env`) plus a bearer-authed
//! `GET /admin/env` readout of the single config-sibling `.env` file,
//! bearer-authed pending-state reads - `GET /admin/config-pending` (the
//! merged real-plus-shadow view in the `GET /admin/config` shape, with a
//! distinct boot side for the restart-required banner) and
//! `GET /admin/config-dirty` (shadow existence, pending files, changed
//! sections) - bearer-authed `POST /admin/config-apply` (promote every
//! shadow to its real file, then reload the active profile, or report
//! restart-required for a promoted boot shadow) and
//! `POST /admin/config-revert` (delete every shadow, touching nothing
//! else), a loopback-only, bearer-authed `POST /admin/reveal` opening the
//! host OS file manager at a path confined to the artifact cache, a
//! loopback-only, bearer-authed `POST /shutdown` driving the same
//! graceful shutdown Ctrl-C drives - and
//! `GET /health`. The whole admin config surface (config read/write, env,
//! pending state, apply/revert, orphans, system, model-info, the HF
//! proxy, reveal, shutdown) sits behind the shared loopback
//! wall from `shared-loopback` in every build; with the
//! `config-ui` feature the embedded config SPA is served at `/config/`
//! behind the same wall, and `GET /auth?key=` sets a session proof
//! derived from the bearer key as an HttpOnly cookie and redirects to the
//! key-free `/config/`, so a browser handoff never leaves the key in
//! browser history. When the listener is
//! bound to loopback, every route additionally sits behind the shared
//! host-authority wall, which refuses requests whose `Host` is not the
//! bound socket (the DNS-rebinding defense). In-process
//! llama.cpp FFI and endpoint pinning are deferred.

mod api_error;
mod boot;
#[cfg(feature = "local")]
mod cache;
#[cfg(feature = "local")]
mod chat_templates;
mod commands;
mod config_apply;
mod config_pending;
mod config_write;
mod dialect;
mod drain;
mod env_file;
mod error;
mod handoff;
mod hf;
#[cfg(feature = "local")]
mod model_info;
#[cfg(feature = "local")]
mod orphans;
mod relaunch;
mod render;
mod reveal;
mod routing;
mod runner;
mod shutdown;
mod system;
#[cfg(test)]
mod test_support;
mod tray;

// The wire protocol and upstream abstraction live in the protocol crate;
// these re-exports keep every `crate::wire::*` and `crate::upstream::*`
// path resolving unchanged.
pub(crate) use shared_protocol::{upstream, wire};
// The dominion admission queues live in the routing crate; this re-export
// keeps every `crate::queue::*` path resolving unchanged.
pub(crate) use gateway_routing::queue;
// Local inference lives in its own crate behind the `local` feature; this
// re-export keeps every `crate::local::*` path resolving unchanged.
#[cfg(feature = "local")]
pub(crate) use gateway_local as local;

pub use crate::api_error::{ServeError, StartupError, StartupErrorKind};
pub use crate::relaunch::running_gateway_settings_url;
pub use crate::runner::{
    Gateway, GatewayHandle, ProfilesContext, ServeOptions, run, run_printing_url, spawn,
};
pub use crate::tray::run_with_tray;
pub use gateway_config::{
    Config, ConfigError, ConfigErrorKind, ProfileName, ProfileNameError, Secret,
};

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
#[cfg(feature = "stt")]
use axum::extract::FromRequest;
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
#[cfg(feature = "local")]
use axum::routing::delete;
use axum::routing::{get, post};
use axum::{Router, response::IntoResponse};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::GatewayError;
#[cfg(feature = "local")]
use crate::local::LocalRuntime;
use crate::routing::Routing;
use crate::wire::{
    ChatRequest, EmbeddingRequest, EmbeddingResponse, ModelInfo, ModelsResponse, RerankRequest,
    RerankResponse,
};
use gateway_config::ModelKind;
#[cfg(feature = "web-search")]
use gateway_config::WebSearchConfig;
#[cfg(feature = "stt")]
use gateway_stt::{SttRuntime, SttState};
#[cfg(feature = "web-search")]
use gateway_web_search::{WebSearchRequest, WebSearchResponse, WebSearchState};
use shared_progress::{EventState, OperationId, ProgressEvent, ProgressHub, ProgressTree};

/// Mutable live configuration held behind a lock so profile switches can swap
/// routing and local children without rebuilding the axum router.
#[derive(Debug)]
struct LiveState {
    routing: Arc<Routing>,
    key: Secret,
    /// The running configuration, retained so `GET /admin/config` can render
    /// it; swapped with the rest of the live state on a profile switch.
    config: Arc<Config>,
    #[cfg(feature = "web-search")]
    web_search: Option<Arc<WebSearchState>>,
    #[cfg(feature = "local")]
    local: LocalRuntime,
    #[cfg(feature = "stt")]
    stt: Option<SttRuntime>,
    profile_name: Option<String>,
    /// The active profile's `models` allowlist, when it declared one.
    model_allowlist: Option<Vec<String>>,
}

/// Single configuration file used by admin routes and profile persistence.
#[derive(Debug)]
struct AdminConfig {
    path: std::path::PathBuf,
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

/// Shared handler state: live routing/key/local runtime, configuration path,
/// and switch coordination.
#[derive(Debug, Clone)]
pub(crate) struct AppState {
    live: Arc<RwLock<LiveState>>,
    config: Option<Arc<AdminConfig>>,
    /// Process-lifetime identifier used by the config UI to detect a restart.
    config_generation: Arc<str>,
    /// Serializes profile switches so two concurrent switches cannot interleave
    /// their reads and writes of the live state. Inference registration takes
    /// this same lock before entering the in-flight set.
    switch: Arc<tokio::sync::Mutex<()>>,
    /// Inference requests that must drain before local children stop.
    in_flight: Arc<drain::InFlight>,
    /// Serializes direct profile switches, `POST /admin/config-apply`,
    /// `POST /admin/config-revert`, and every shadow-writing `PUT` save, so
    /// state persistence cannot race pending-state promotion and Apply only
    /// promotes shadow combinations the latest save validated whole.
    apply: Arc<tokio::sync::Mutex<()>>,
    /// The process-lifetime progress broker: operations attach trees for
    /// their own lifetimes, and `GET /admin/progress` streams its events.
    hub: Arc<ProgressHub>,
    /// The command queue: boot provisioning, profile switches, and unloads
    /// run as serialized, cancellable commands; the tray and routes read its
    /// status in-process.
    commands: commands::CommandQueue,
    /// Shared host-metrics sampler for `GET /admin/system`: one process-wide
    /// `sysinfo::System` so CPU-utilization deltas span requests, plus the
    /// once-per-process NVML probe.
    metrics: Arc<std::sync::Mutex<system::SystemSampler>>,
    /// Shared Hugging Face hub client for the `GET /admin/hf/*` proxy
    /// routes: one reqwest client plus the boot-time `HF_TOKEN`.
    hf: Arc<hf::HfProxy>,
    /// Launches the OS file manager for `POST /admin/reveal`; injectable
    /// so tests assert the constructed command without spawning anything.
    reveal: Arc<dyn reveal::RevealLauncher>,
    /// The process-shutdown signal fired by `POST /shutdown`; the serve
    /// loop selects on it alongside the caller-owned shutdown future.
    shutdown: shutdown::ShutdownSignal,
    /// Process-lifetime random salt for the `/auth` handoff's session
    /// proof; a restart or key rotation invalidates every minted cookie.
    handoff_salt: [u8; 32],
    /// Stable STT slot shared across runtime replacement on a profile
    /// switch.
    #[cfg(feature = "stt")]
    stt_state: SttState,
}

impl AppState {
    /// Build full runtime state for `Gateway` and integration tests.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the single caller assembles process state; a parameter struct would invent a grouping with no domain meaning"
    )]
    pub(crate) fn from_parts(
        routing: Arc<Routing>,
        key: Secret,
        config: Arc<Config>,
        #[cfg(feature = "local")] local: LocalRuntime,
        #[cfg(feature = "stt")] stt: SttRuntime,
        #[cfg(feature = "web-search")] web_search: Option<&WebSearchConfig>,
        config_path: Option<std::path::PathBuf>,
        selection: ProfileSelection,
        hub: Arc<ProgressHub>,
    ) -> AppState {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        #[cfg(feature = "stt")]
        let stt_state = stt.state();
        AppState {
            live: Arc::new(RwLock::new(LiveState {
                routing,
                key,
                config,
                #[cfg(feature = "web-search")]
                web_search: web_search.map(|cfg| Arc::new(WebSearchState::new(cfg))),
                #[cfg(feature = "local")]
                local,
                #[cfg(feature = "stt")]
                stt: Some(stt),
                profile_name: selection.name,
                model_allowlist: selection.model_allowlist,
            })),
            config: config_path.map(|path| Arc::new(AdminConfig { path })),
            config_generation: format!("{}-{started}", std::process::id()).into(),
            switch: Arc::new(tokio::sync::Mutex::new(())),
            in_flight: Arc::new(drain::InFlight::default()),
            apply: Arc::new(tokio::sync::Mutex::new(())),
            commands: commands::CommandQueue::new(Arc::clone(&hub)),
            hub,
            metrics: Arc::new(std::sync::Mutex::new(system::SystemSampler::new())),
            hf: Arc::new(hf::HfProxy::from_env()),
            reveal: Arc::new(reveal::SpawnLauncher),
            shutdown: shutdown::ShutdownSignal::default(),
            handoff_salt: {
                // The OS-seeded CSPRNG, as for the generated bearer key:
                // the salt keeps a harvested handoff cookie from ever
                // resolving to the long-term key.
                use rand::Rng as _;
                let mut salt = [0u8; 32];
                rand::rng().fill(&mut salt);
                salt
            },
            #[cfg(feature = "stt")]
            stt_state,
        }
    }

    /// The web-search capability, when configured.
    #[cfg(feature = "web-search")]
    pub(crate) async fn web_search(&self) -> Option<Arc<WebSearchState>> {
        self.live.read().await.web_search.clone()
    }

    /// The active profile's `[local].cache_dir` setting, for the cache routes.
    #[cfg(feature = "local")]
    pub(crate) async fn cache_dir(&self) -> Option<String> {
        self.live.read().await.local.cache_dir().map(str::to_owned)
    }

    /// Registers an inference request under the same lock profile switches use.
    async fn begin_inference(&self) -> drain::InFlightGuard {
        let _switch = self.switch.lock().await;
        self.in_flight.register()
    }

    /// A point-in-time readout for the tray's status line: the number of
    /// models in the live routing table and the declared VRAM total of the
    /// active local and STT models.
    ///
    /// Returns `None` when a profile switch holds the live-state write
    /// lock: the tray's timer skips that tick rather than blocking the
    /// message loop.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", test))]
    pub(crate) fn tray_model_status(&self) -> Option<(usize, f64)> {
        let live = self.live.try_read().ok()?;
        let models = live.routing.models().len();
        let vram_gb = live
            .config
            .local_models()
            .iter()
            .filter_map(gateway_config::LocalModelConfig::vram_gb)
            .sum::<f64>()
            + live
                .config
                .stt_models()
                .iter()
                .map(gateway_config::SttModelConfig::vram_gb)
                .sum::<f64>();
        Some((models, vram_gb))
    }
}

/// Build the gateway's axum router.
///
/// `bound` is the socket the server actually bound. When it is loopback,
/// the whole surface is wrapped in the shared host-authority wall
/// ([`shared_loopback::require_loopback_host`]), the DNS-rebinding
/// defense; a non-loopback bind installs nothing, since a LAN server has
/// no loopback allowlist to enforce. The [`Gateway::router`] seam passes
/// `None` and carries no host wall: with no bound socket there is no
/// authority to allowlist.
pub(crate) fn build_router(state: AppState, bound: Option<std::net::SocketAddr>) -> Router {
    let router = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/rerank", post(rerank))
        .route("/v1/models", get(list_models))
        .route("/health", get(health))
        .route("/admin/profiles", get(admin_list_profiles))
        .route("/admin/status", get(admin_status))
        .route("/admin/progress", get(admin_progress))
        .route("/admin/switch-profile", post(admin_switch_profile));
    #[cfg(feature = "stt")]
    let router = router.merge(
        Router::new()
            .route("/v1/audio/transcriptions", post(audio_transcriptions))
            .layer(axum::extract::DefaultBodyLimit::max(
                gateway_stt::MAX_AUDIO_BYTES + 1024 * 1024,
            )),
    );
    // The web-search tool route delegates to the service crate, so it exists
    // only in builds with the `web-search` feature.
    #[cfg(feature = "web-search")]
    let router = router.route("/v1/tools/web_search", post(web_search));
    // The blob-cache routes serve the local artifact store, so they exist
    // only in builds with local inference.
    #[cfg(feature = "local")]
    let router = router
        .route("/v1/cache", get(cache::list_cache).post(cache::post_cache))
        .route("/v1/cache/{sha256}", delete(cache::delete_cache));

    // The admin config surface reads secrets in plaintext, writes files,
    // and launches processes, so every route below sits behind the shared
    // loopback wall in every build: a non-loopback peer is refused with
    // 403 before bearer auth even runs. `POST /shutdown` kills the process
    // and `GET /auth` mints the key's ambient cookie, so both are walled
    // with the config surface they serve.
    let walled = Router::new()
        .route("/shutdown", post(shutdown::admin_shutdown))
        .route("/admin/system", get(system::admin_system))
        .route(
            "/admin/config",
            get(admin_config).put(config_write::admin_put_config),
        )
        .route(
            "/admin/config-pending",
            get(config_pending::admin_config_pending),
        )
        .route(
            "/admin/config-dirty",
            get(config_pending::admin_config_dirty),
        )
        .route(
            "/admin/config-apply",
            post(config_apply::admin_config_apply),
        )
        .route(
            "/admin/config-revert",
            post(config_apply::admin_config_revert),
        )
        .route(
            "/admin/env",
            get(env_file::admin_get_env).put(env_file::admin_put_env),
        )
        .route("/admin/reveal", post(reveal::admin_reveal))
        .route("/admin/hf/search", get(hf::admin_hf_search))
        .route("/admin/hf/model/{owner}/{name}", get(hf::admin_hf_model))
        .route(
            "/admin/hf/model/{owner}/{name}/readme",
            get(hf::admin_hf_readme),
        );
    // The template, orphan, and model-info routes read local-inference
    // facilities, so they exist only in builds with local inference.
    #[cfg(feature = "local")]
    let walled = walled
        .route(
            "/admin/chat-templates",
            get(chat_templates::admin_chat_templates),
        )
        .route("/admin/orphans", get(orphans::admin_orphans))
        .route("/admin/model-info", get(model_info::admin_model_info));
    // `GET /config` (no trailing slash) redirects to `/config/` so the
    // SPA's relative asset references resolve against the mount point;
    // it is walled like the assets it fronts. `GET /auth` is the browser
    // handoff onto that surface, so it exists only when the surface does.
    #[cfg(feature = "config-ui")]
    let walled = walled
        .route("/config", get(config_ui_redirect))
        .route("/auth", get(handoff::auth_handoff));
    let router = router
        .merge(walled.route_layer(axum::middleware::from_fn(shared_loopback::require_loopback)));
    // The SPA asset router arrives with the same loopback wall already
    // applied inside `routes()`; `nest_service` because the asset router
    // carries no gateway state.
    #[cfg(feature = "config-ui")]
    let router = router.nest_service("/config/", gateway_config_ui::routes());
    let router = router.with_state(state);
    // The host-authority wall is the outermost layer, so a rebound
    // hostname is refused before any route logic runs.
    match bound {
        Some(bound) => router.layer(axum::middleware::from_fn_with_state(
            bound,
            shared_loopback::require_loopback_host,
        )),
        None => router,
    }
}

/// Redirects `GET /config` to `/config/`, where the SPA index is served
/// and its relative asset references resolve.
#[cfg(feature = "config-ui")]
async fn config_ui_redirect() -> axum::response::Redirect {
    axum::response::Redirect::permanent("/config/")
}

/// The `POST /v1/tools/web_search` route: bearer-authed, delegates to the
/// web-search service crate.
///
/// # Errors
/// Returns [`GatewayError::Unauthorized`] when the bearer token is absent or
/// wrong, [`GatewayError::ToolNotConfigured`] when no `[tools.web_search]`
/// section is present, [`GatewayError::MalformedRequest`] when the request
/// fails validation, and the upstream variants on a provider failure.
#[cfg(feature = "web-search")]
async fn web_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebSearchRequest>,
) -> Result<Json<WebSearchResponse>, GatewayError> {
    check_auth(&state, &headers).await?;
    let service = state
        .web_search()
        .await
        .ok_or(GatewayError::ToolNotConfigured("web_search"))?;
    Ok(Json(service.search(&request).await?))
}

/// Liveness probe; unauthenticated and always 200 while serving.
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "serving" }))
}

/// OpenAI-compatible request-response transcription.
///
/// Authentication runs before multipart extraction so an unauthorized caller
/// cannot make the gateway buffer or decode an audio body.
#[cfg(feature = "stt")]
async fn audio_transcriptions(
    State(state): State<AppState>,
    request: axum::extract::Request,
) -> Result<Response, GatewayError> {
    check_auth(&state, request.headers()).await?;
    let multipart = axum::extract::Multipart::from_request(request, &())
        .await
        .map_err(|error| GatewayError::MalformedRequest(error.to_string()))?;
    let in_flight = state.begin_inference().await;
    tokio::select! {
        result = gateway_stt::transcribe(&state.stt_state, multipart) => {
            result.map(IntoResponse::into_response).map_err(GatewayError::from)
        }
        () = in_flight.cancelled() => Err(GatewayError::RequestCancelled),
    }
}

/// Header naming the caller for fair queue scheduling. Absent → `"default"`.
const CLIENT_HEADER: &str = "X-PromptForge-Client";

/// Error message when a configuration declaring `[[local_model]]` reaches a
/// build compiled without the `local` feature.
#[cfg(not(feature = "local"))]
const LOCAL_MODELS_UNSUPPORTED: &str =
    "configuration declares [[local_model]] but this build lacks the `local` feature";

/// Error when STT reaches a gateway build without the heavy runtime.
#[cfg(not(feature = "stt"))]
const STT_RUNTIME_UNAVAILABLE: &str =
    "the active profile selects [[stt_model]] but this build lacks the `stt` feature";

/// Resolves a request's model name against the live routing table.
///
/// A configured but not-yet-loaded model - one the catalog names while the
/// routing table is still empty or mid-switch - earns a 503 naming the
/// active queue command rather than a bare 404, so the caller knows to
/// retry once the command completes. With no command active the miss is
/// [`GatewayError::UnknownModel`], exactly as before the queue existed.
async fn resolve_routed_model(
    state: &AppState,
    name: &str,
) -> Result<Arc<crate::routing::Model>, GatewayError> {
    let live = state.live.read().await;
    match live.routing.model(name) {
        Ok(model) => Ok(model),
        Err(unknown) => {
            let configured = live
                .config
                .catalog_models()
                .iter()
                .any(|model| model.name() == name)
                || live
                    .config
                    .catalog_local_models()
                    .iter()
                    .any(|model| model.name() == name);
            if configured && let Some(active) = state.commands.active_command() {
                return Err(GatewayError::ModelProvisioning(active.name));
            }
            Err(unknown)
        }
    }
}

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
    let in_flight = state.begin_inference().await;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Chat)?;
    let client_id = crate::queue::ClientId::from_header(
        headers
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let permit = tokio::select! {
        result = model.endpoint.queue.admit(client_id.as_str()) => result?,
        () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
    };
    // Emulated dialects rewrite the request (guide injection, tool stripping)
    // and parse the reply's content fences. The fence parse needs the whole
    // reply, so the emulated streaming path buffers one non-streaming
    // upstream round trip and re-emits the rewritten response as synthetic
    // chunks; without it an always-streaming caller would silently lose
    // tool calling on this dialect.
    let emulated = model.tool_dialect == crate::dialect::GEMMA3_TOOL_CODE;
    let request = if emulated {
        let mut request = request;
        crate::dialect::prepare_request(&mut request)?;
        request
    } else {
        request
    };
    if request.stream {
        if emulated {
            let mut buffered = request;
            buffered.stream = false;
            // Streaming-only options must not reach a non-streaming upstream
            // call; the synthetic summary chunk restores the usage the
            // caller asked `stream_options.include_usage` for.
            buffered.rest.remove("stream_options");
            let response = tokio::select! {
                result = model.endpoint.upstream.send(buffered, &model.upstream_name) => result?,
                () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
            };
            response
                .validate()
                .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
            let mut response = response;
            crate::dialect::apply_response(&mut response, &model.name);
            return Ok(relay_sse(
                crate::dialect::response_chunks(response),
                permit,
                in_flight,
            ));
        }
        // A failure here is before the SSE response starts, so it is
        // consumed as a normal JSON error, never a stream that dies
        // mid-flight.
        let streamed = tokio::select! {
            result = model.endpoint.upstream.stream(request, &model.upstream_name) => result?,
            () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
        };
        return Ok(relay_sse(streamed, permit, in_flight));
    }
    let response = tokio::select! {
        result = model.endpoint.upstream.send(request, &model.upstream_name) => result?,
        () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
    };
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
fn relay_sse(
    streamed: crate::upstream::StreamedChunks,
    permit: crate::queue::Permit,
    in_flight: drain::InFlightGuard,
) -> Response {
    use futures_util::StreamExt as _;

    let relayed = futures_util::stream::unfold(
        (streamed.chunks, false, permit, in_flight),
        |(mut chunks, failed, permit, in_flight)| async move {
            if failed {
                return None;
            }
            let (line, failed) = tokio::select! {
                item = chunks.next() => {
                    let item = item?;
                    match item {
                        Ok(chunk) => match serde_json::to_string(&chunk) {
                            Ok(json) => (format!("data: {json}\n\n"), false),
                            Err(error) => (
                                format!(
                                    "data: {}\n\n",
                                    GatewayError::upstream_protocol(error).envelope()
                                ),
                                true,
                            ),
                        },
                        Err(error) => (format!("data: {}\n\n", error.envelope()), true),
                    }
                }
                () = in_flight.cancelled() => (
                    format!("data: {}\n\n", GatewayError::RequestCancelled.envelope()),
                    true,
                ),
            };
            Some((
                Ok::<String, std::convert::Infallible>(line),
                (chunks, failed, permit, in_flight),
            ))
        },
    );
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
    let in_flight = state.begin_inference().await;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Embedding)?;
    let client_id = crate::queue::ClientId::from_header(
        headers
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let _permit = tokio::select! {
        result = model.endpoint.queue.admit(client_id.as_str()) => result?,
        () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
    };
    let response = tokio::select! {
        result = model.endpoint.upstream.send_embeddings(request, &model.upstream_name) => result?,
        () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
    };
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
    let in_flight = state.begin_inference().await;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Classifier)?;
    let client_id = crate::queue::ClientId::from_header(
        headers
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let _permit = tokio::select! {
        result = model.endpoint.queue.admit(client_id.as_str()) => result?,
        () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
    };
    let response = tokio::select! {
        result = model.endpoint.upstream.send_rerank(request, &model.upstream_name) => result?,
        () = in_flight.cancelled() => return Err(GatewayError::RequestCancelled),
    };
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

/// Lists profile names from the loaded global catalog.
async fn admin_list_profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let live = state.live.read().await;
    let profiles: Vec<&str> = live
        .config
        .profiles()
        .iter()
        .map(gateway_config::ProfileConfig::name)
        .collect();
    Ok(Json(serde_json::json!({ "profiles": profiles })))
}

/// Current profile name, loaded model names, process config generation,
/// the profile's model allowlist, and a queue note.
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
    // A headless build has no local runtime; it reports zero children rather
    // than dropping the field from the status response.
    #[cfg(feature = "local")]
    let local_children = live.local.child_count();
    #[cfg(not(feature = "local"))]
    let local_children = 0;
    Ok(Json(serde_json::json!({
        "profile": live.profile_name,
        "models": models,
        "config_generation": state.config_generation.as_ref(),
        "model_allowlist": live.model_allowlist,
        "local_children": local_children,
        "queue": "per-dominion shared waiting queue; profile switch drains in-flight inference for up to 30 seconds",
    })))
}

/// The `GET /admin/config` route: bearer-authed, renders the running global
/// config plus its active profile in the pending admin shape.
async fn admin_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let live = state.live.read().await;
    let mut document = live.config.to_json();
    if let Some(table) = document.as_object_mut()
        && let Some(profile) = live.config.active_profile()
    {
        table.insert(
            "active_profile".to_owned(),
            serde_json::Value::String(profile.name().to_owned()),
        );
    }
    Ok(Json(document))
}

/// Heartbeat cadence for the progress stream: SSE comment lines keep an
/// idle connection alive through NAT and firewall timeouts.
const PROGRESS_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);

/// The `GET /admin/progress` route: bearer-authed, streams the process
/// progress hub as SSE.
///
/// The reply is `text/event-stream` and never terminates on its own: a
/// freshly connected subscriber first receives the live operations replayed
/// as synthetic `Begun`/`Updated` events, plus a `Finished` for each leaf
/// that already reached its terminal state, so it can render current state
/// without waiting for the next event, and then every broadcast
/// [`ProgressEvent`], with heartbeat comment lines every
/// [`PROGRESS_HEARTBEAT`] while the hub is idle. Intermediate events are
/// lossy - a lagging subscriber drops them - and terminal events are never
/// coalesced at the source. Client disconnect is Drop all the way down, as
/// with the switch stream: the response body owns the receiver.
async fn admin_progress(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    Ok(progress_sse_response(&state.hub))
}

/// Builds the progress SSE response over `hub`: a snapshot of the live
/// operations first, then the broadcast stream, heartbeats in the gaps.
fn progress_sse_response(hub: &ProgressHub) -> Response {
    // Subscribe before snapshotting so no event between the two is lost; a
    // `Begun` replayed from the snapshot is idempotent for remote import.
    let rx = hub.subscribe();
    let mut pending = std::collections::VecDeque::new();
    for operation in hub.snapshot() {
        for node in &operation.nodes {
            pending.extend(event_line(&ProgressEvent::new(
                operation.operation,
                node.path.clone(),
                node.label.clone(),
                EventState::Begun {
                    weight: node.weight,
                },
            )));
            if node.fraction > 0.0 {
                pending.extend(event_line(&ProgressEvent::new(
                    operation.operation,
                    node.path.clone(),
                    node.label.clone(),
                    EventState::Updated {
                        fraction: node.fraction,
                    },
                )));
            }
            // A leaf that finished before the subscriber connected replays
            // its terminal event too, or the subscriber would hold it as
            // unfinished until the tree detaches.
            if node.finished {
                pending.extend(event_line(&ProgressEvent::new(
                    operation.operation,
                    node.path.clone(),
                    node.label.clone(),
                    EventState::Finished { ok: node.ok },
                )));
            }
        }
    }
    let heartbeat_at = tokio::time::Instant::now() + PROGRESS_HEARTBEAT;
    let stream = futures_util::stream::unfold(
        (
            pending,
            rx,
            tokio::time::interval_at(heartbeat_at, PROGRESS_HEARTBEAT),
        ),
        |(mut pending, mut rx, mut heartbeat)| async move {
            if let Some(line) = pending.pop_front() {
                return Some((
                    Ok::<_, std::convert::Infallible>(line),
                    (pending, rx, heartbeat),
                ));
            }
            loop {
                tokio::select! {
                    _ = heartbeat.tick() => {
                        return Some((Ok(": heartbeat\n\n".to_owned()), (pending, rx, heartbeat)));
                    }
                    received = rx.recv() => match received {
                        Ok(event) => {
                            if let Some(line) = event_line(&event) {
                                return Some((Ok(line), (pending, rx, heartbeat)));
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "progress subscriber lagged; events dropped");
                        }
                        // The hub lives in `AppState` for the process
                        // lifetime, so its sender never closes first.
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    },
                }
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// Serializes one event as an SSE `data:` line, or `None` (logged) when
/// serialization fails: the wire types are plain data, so a failure is a
/// schema bug, and one bad event must not kill the stream.
fn event_line(event: &ProgressEvent) -> Option<String> {
    match serde_json::to_string(event) {
        Ok(json) => Some(format!("data: {json}\n\n")),
        Err(error) => {
            tracing::warn!(%error, "progress event failed to serialize; dropping it");
            None
        }
    }
}

/// Immediately switches to another named profile, streaming its progress.
///
/// The reply is `text/event-stream`: a `{"stage": ...}` event opens each
/// phase in execution order - `loading-profile` around config load and
/// validation, `stopping-models` before the old local children shut down,
/// `starting-models` before the new children load their weights into VRAM
/// (the long pole) - and the stream ends with exactly one terminal event,
/// `{"status": "ready", "profile": ...}` or `{"status": "error",
/// "message": ...}`. The bounded drain has no stage of its own, preserving
/// the existing stage vocabulary. A refusal before the switch starts (bad
/// auth or a malformed name) stays a buffered JSON error envelope. Builds
/// without the `local` feature emit no `stopping-models`/`starting-models`
/// stages, and refuse a profile declaring `[[local_model]]` with a terminal
/// error event instead of starting children.
///
/// The switch runs as a `LoadProfile` command on the gateway's command
/// queue: serialized with every other command, debounced so a burst of
/// switches runs only the latest, and cancellable through the command's
/// token. A client disconnect drops only the response body and its hub
/// subscription, never the half-finished command.
async fn admin_switch_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SwitchProfileRequest>,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    let name = ProfileName::parse(&request.name)
        .map_err(|e| GatewayError::switch_failed("parse-name", e))?;
    // Subscribe before enqueueing so no event of this switch is missed; the
    // response filters the hub stream to this command's operation.
    let rx = state.hub.subscribe();
    let enqueued = state.commands.enqueue(commands::Command::load_profile(
        name,
        true,
        tokio_util::sync::CancellationToken::new(),
    ));
    let switch = tokio::spawn(async move {
        enqueued.outcome.await.unwrap_or_else(|_| {
            // The worker settles every command it begins, so a dropped
            // sender means the worker task itself died.
            Arc::new(Err(GatewayError::switch_failed(
                "queue",
                std::io::Error::other("the command queue dropped the command without settling it"),
            )))
        })
    });
    Ok(switch_sse_response(rx, enqueued.operation, switch))
}

/// How a successful switch commits its active-profile state.
pub(crate) enum StatePersistence {
    /// The selection already matches persisted state.
    None,
    /// Atomically replace real state while preserving any pending shadow.
    Write,
    /// Promote the pending state shadow as part of Apply.
    Promote(std::path::PathBuf),
}

/// Executes a switch using an optional catalog parsed by Apply.
///
/// `token` is the command's cancellation: checked at phase boundaries and
/// honored by the local start, so a cancelled switch stops instead of
/// running its remaining phases. Apply passes a never-fired token.
/// `persistence` is evaluated once, at commit time, so a debounced queue
/// duplicate can upgrade an ephemeral load into a persisted one while the
/// switch is still running.
async fn run_switch_with_config(
    state: AppState,
    name: ProfileName,
    tree: ProgressTree,
    candidate: Option<Config>,
    persistence: impl FnOnce() -> StatePersistence,
    token: &tokio_util::sync::CancellationToken,
) -> Result<String, GatewayError> {
    // Serialize switches for the whole operation (LIB-008).
    let _switch = state.switch.lock().await;

    // A cancelled command stops at phase boundaries rather than midway.
    if token.is_cancelled() {
        return Err(GatewayError::CommandCancelled(format!(
            "load-profile: {name}"
        )));
    }
    // Each phase registers its leaf as it opens, so the leaf's `Begun` is the
    // stage marker and a failed switch never announces a phase it did not
    // reach. Weights track expected duration: starting the new children is
    // the long pole.
    let loading = tree.register("loading-profile", 1.0);
    let catalog = match candidate {
        Some(config) => config,
        None => state.live.read().await.config.as_ref().clone(),
    };
    let (config, remote_routing) = prepare_switch_target(&catalog, &name, &loading)?;
    loading.complete();

    #[cfg(feature = "web-search")]
    let new_web_search = config
        .web_search_config()
        .map(WebSearchState::new)
        .map(Arc::new);
    let new_allowlist = config
        .active_profile()
        .map(|profile| profile.models().to_vec());

    if token.is_cancelled() {
        return Err(GatewayError::CommandCancelled(format!(
            "load-profile: {name}"
        )));
    }
    // A cancelled command stops waiting on the drain: the replacement
    // switch (or the shutdown path) re-drains what it needs.
    tokio::select! {
        () = drain_inference(&state) => {}
        () = token.cancelled() => {
            return Err(GatewayError::CommandCancelled(format!(
                "load-profile: {name}"
            )));
        }
    }

    // Stop every previous VRAM owner before starting replacements. The bearer
    // key, routing, and web-search settings stay untouched, so auth remains
    // stable if a start fails.
    #[cfg(any(feature = "local", feature = "stt"))]
    let replacement = replace_runtimes(&state, &config, &tree, token).await?;
    // Phase boundary: a token fired during the start stops before the
    // persist and the swap; dropping the replacement tears down any
    // children it started.
    if token.is_cancelled() {
        return Err(GatewayError::CommandCancelled(format!(
            "load-profile: {name}"
        )));
    }
    // A headless build cannot honor a profile declaring local models; refuse
    // the switch rather than silently dropping them.
    #[cfg(not(feature = "local"))]
    if !config.local_models().is_empty() {
        return Err(GatewayError::switch_failed(
            "start-local",
            std::io::Error::other(LOCAL_MODELS_UNSUPPORTED),
        ));
    }

    #[cfg(feature = "local")]
    let routing = remote_routing
        .merge(replacement.local.models().iter().cloned())
        .map_err(|e| GatewayError::switch_failed("merge-routing", e))?;
    #[cfg(not(feature = "local"))]
    let routing = remote_routing;

    // Persistence is part of the serialized switch. Once the state file
    // commits, the in-memory swap below is infallible, so another switch can
    // never overwrite pending state between activation and persistence.
    commit_profile_state(&state, &name, persistence()).await?;

    // Atomic swap: commit the whole new profile at once.
    {
        let mut live = state.live.write().await;
        live.routing = Arc::new(routing);
        // The listener and bearer key are process-owned `[server]` state.
        // Apply reports their edits as restart-required, so a profile reload
        // must not change authentication before that restart.
        live.config = Arc::new(config);
        #[cfg(feature = "web-search")]
        {
            live.web_search = new_web_search;
        }
        #[cfg(feature = "local")]
        {
            live.local = replacement.local;
        }
        #[cfg(feature = "stt")]
        {
            live.stt = Some(replacement.stt);
        }
        live.profile_name = Some(name.to_string());
        live.model_allowlist = new_allowlist;
    }

    #[cfg(feature = "local")]
    if !replacement.start_failures.is_empty() {
        let loaded = state
            .live
            .read()
            .await
            .local
            .models()
            .iter()
            .map(|model| model.name.clone())
            .collect();
        let failed = replacement
            .start_failures
            .into_iter()
            .map(|failure| format!("{}: {}", failure.model(), failure.error()))
            .collect();
        return Err(GatewayError::PartialStart {
            profile: name.to_string(),
            loaded,
            failed,
        });
    }

    tracing::info!(profile = %name, "switched profile");
    Ok(name.to_string())
}

fn prepare_switch_target(
    catalog: &Config,
    name: &ProfileName,
    loading: &shared_progress::ProgressHandle,
) -> Result<(Config, Routing), GatewayError> {
    if !catalog
        .profiles()
        .iter()
        .any(|profile| profile.name() == name.as_str())
    {
        loading.fail();
        return Err(GatewayError::ProfileNotFound(name.to_string()));
    }
    let config = match catalog.select_profile(name) {
        Ok(config) => config,
        Err(error) => {
            loading.fail();
            return Err(GatewayError::switch_failed("select-profile", error));
        }
    };
    #[cfg(not(feature = "stt"))]
    if !config.stt_models().is_empty() {
        loading.fail();
        return Err(GatewayError::switch_failed(
            "start-stt",
            std::io::Error::other(STT_RUNTIME_UNAVAILABLE),
        ));
    }
    let remote_routing = match Routing::from_config(&config) {
        Ok(routing) => routing,
        Err(error) => {
            loading.fail();
            return Err(GatewayError::switch_failed("build-routing", error));
        }
    };
    Ok((config, remote_routing))
}

async fn drain_inference(state: &AppState) {
    if !state
        .in_flight
        .drain_or_cancel(std::time::Duration::from_secs(30))
        .await
    {
        tracing::warn!(
            "profile-switch cancellation grace expired; stopping local children with request guards still registered"
        );
    }
}

#[cfg(any(feature = "local", feature = "stt"))]
struct RuntimeReplacement {
    #[cfg(feature = "local")]
    local: LocalRuntime,
    #[cfg(feature = "local")]
    start_failures: Vec<local::LocalStartFailure>,
    #[cfg(feature = "stt")]
    stt: SttRuntime,
}

#[cfg(any(feature = "local", feature = "stt"))]
#[expect(
    clippy::too_many_lines,
    reason = "the stop-then-start phase shape repeats per runtime behind cfg gates; splitting it would multiply the cfg matrix across call sites"
)]
async fn replace_runtimes(
    state: &AppState,
    config: &Config,
    tree: &ProgressTree,
    token: &tokio_util::sync::CancellationToken,
) -> Result<RuntimeReplacement, GatewayError> {
    let stopping = tree.register("stopping-models", 2.0);
    #[cfg(feature = "local")]
    let old_local = {
        let mut live = state.live.write().await;
        std::mem::replace(&mut live.local, LocalRuntime::empty())
    };
    #[cfg(feature = "stt")]
    let old_stt = state.live.write().await.stt.take();
    // The routing table also owns each local upstream. Explicit shutdown
    // disables respawn and frees all old-profile VRAM before replacements
    // start (PFGL-MOD-001).
    match tokio::task::spawn_blocking(move || {
        #[cfg(feature = "stt")]
        if let Some(runtime) = old_stt {
            runtime.shutdown();
        }
        #[cfg(feature = "local")]
        {
            let result = old_local.shutdown();
            drop(old_local);
            result
        }
        #[cfg(not(feature = "local"))]
        Ok::<(), std::convert::Infallible>(())
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            stopping.fail();
            return Err(GatewayError::switch_failed("shutdown-local", error));
        }
        Err(error) => {
            stopping.fail();
            return Err(GatewayError::switch_failed("shutdown-local-task", error));
        }
    }

    stopping.complete();
    // Phase boundary: start no replacement children for a cancelled command.
    if token.is_cancelled() {
        return Err(GatewayError::CommandCancelled("profile switch".to_owned()));
    }
    let starting = tree.register("starting-models", 5.0);
    #[cfg(feature = "local")]
    let start_config = config.clone();
    #[cfg(feature = "local")]
    let start_progress = starting.clone();
    #[cfg(feature = "local")]
    let outcome = {
        // The child readiness poll predates the token and speaks
        // `AtomicBool`; the bridge task folds the token into the flag so one
        // cancellation source stops a child still loading weights.
        let start_token = token.clone();
        let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let bridge = tokio::spawn({
            let interrupted = Arc::clone(&interrupted);
            let token = token.clone();
            async move {
                token.cancelled().await;
                interrupted.store(true, std::sync::atomic::Ordering::Release);
            }
        });
        let result = tokio::task::spawn_blocking(move || {
            local::LocalRuntime::start_partial_with_cancellation(
                &start_config,
                Some(&start_progress),
                &start_token,
                &interrupted,
            )
        })
        .await;
        bridge.abort();
        match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                starting.fail();
                return Err(GatewayError::switch_failed("start-local", error));
            }
            Err(error) => {
                starting.fail();
                return Err(GatewayError::switch_failed("start-local-task", error));
            }
        }
    };
    #[cfg(feature = "local")]
    let (runtime, failures) = outcome.into_parts();
    // Phase boundary: a cancelled command starts no STT runtime behind the
    // cancellation; the local runtime built above drops, killing its
    // children.
    #[cfg(feature = "stt")]
    if token.is_cancelled() {
        return Err(GatewayError::CommandCancelled("profile switch".to_owned()));
    }
    #[cfg(feature = "stt")]
    let stt_config = config.clone();
    #[cfg(feature = "stt")]
    let stt_state = state.stt_state.clone();
    #[cfg(feature = "stt")]
    let stt_progress = starting.clone();
    #[cfg(feature = "stt")]
    let stt = match tokio::task::spawn_blocking(move || {
        SttRuntime::start(&stt_config, stt_state, Some(&stt_progress))
    })
    .await
    {
        Ok(Ok(runtime)) => runtime,
        Ok(Err(error)) => {
            starting.fail();
            return Err(GatewayError::switch_failed("start-stt", error));
        }
        Err(error) => {
            starting.fail();
            return Err(GatewayError::switch_failed("start-stt-task", error));
        }
    };
    #[cfg(feature = "local")]
    if failures.is_empty() {
        starting.complete();
    } else {
        starting.fail();
    }
    #[cfg(not(feature = "local"))]
    starting.complete();
    Ok(RuntimeReplacement {
        #[cfg(feature = "local")]
        local: runtime,
        #[cfg(feature = "local")]
        start_failures: failures,
        #[cfg(feature = "stt")]
        stt,
    })
}

/// Commits active-profile state while the caller holds the switch lock.
async fn commit_profile_state(
    state: &AppState,
    name: &ProfileName,
    persistence: StatePersistence,
) -> Result<(), GatewayError> {
    match persistence {
        StatePersistence::None => Ok(()),
        StatePersistence::Write => persist_active_profile(state, name).await,
        StatePersistence::Promote(state_path) => {
            tokio::task::spawn_blocking(move || gateway_config::promote_shadow(&state_path))
                .await
                .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))?
                .map_err(config_write::config_write_error)
        }
    }
}

/// Persists the active profile beside the single configuration file.
async fn persist_active_profile(state: &AppState, name: &ProfileName) -> Result<(), GatewayError> {
    let Some(config) = state.config.as_ref() else {
        return Ok(());
    };
    let config_path = config.path.clone();
    let name = name.clone();
    tokio::task::spawn_blocking(move || gateway_config::persist_profile_state(&config_path, &name))
        .await
        .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))?
        .map_err(config_write::config_write_error)
}

/// Builds the switch-profile SSE response: the hub's event stream filtered
/// to this switch's operation, each leaf's `Begun` re-emitted as the
/// `{"stage": ...}` event the route has always carried, then the terminal
/// event from the command's settled outcome, so the outcome can never be
/// lost to broadcast lag.
///
/// The queue worker broadcasts every stage event before settling the
/// command, so once the outcome resolves the remaining stages are already
/// queued on the receiver and are drained ahead of the terminal event.
fn switch_sse_response(
    rx: tokio::sync::broadcast::Receiver<ProgressEvent>,
    operation: OperationId,
    switch: tokio::task::JoinHandle<commands::SharedOutcome>,
) -> Response {
    let stream = futures_util::stream::unfold(
        (rx, switch, std::collections::VecDeque::new(), false),
        move |(mut rx, mut switch, mut pending, mut done)| async move {
            loop {
                if let Some(line) = pending.pop_front() {
                    return Some((
                        Ok::<_, std::convert::Infallible>(line),
                        (rx, switch, pending, done),
                    ));
                }
                if done {
                    return None;
                }
                let result = loop {
                    tokio::select! {
                        received = rx.recv() => match received {
                            Ok(event) => {
                                if event.operation == operation
                                    && matches!(event.state, EventState::Begun { .. })
                                {
                                    return Some((
                                        Ok(stage_line(&event)),
                                        (rx, switch, pending, done),
                                    ));
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::debug!(skipped, "switch stage subscriber lagged; events dropped");
                            }
                            // The hub lives in `AppState` for the process
                            // lifetime, so its sender never closes first; the
                            // join result still carries the outcome if it did.
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                break (&mut switch).await;
                            }
                        },
                        result = &mut switch => break result,
                    }
                };
                drain_switch_stages(&mut rx, operation, &mut pending);
                done = true;
                pending.push_back(terminal_line(result));
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// Drains stage events already queued when the switch task completes.
///
/// A lag marker describes dropped older events, not an empty receiver, so
/// catch-up continues after it and preserves every retained stage.
fn drain_switch_stages(
    rx: &mut tokio::sync::broadcast::Receiver<ProgressEvent>,
    operation: OperationId,
    pending: &mut std::collections::VecDeque<String>,
) {
    loop {
        match rx.try_recv() {
            Ok(event)
                if event.operation == operation
                    && matches!(event.state, EventState::Begun { .. }) =>
            {
                pending.push_back(stage_line(&event));
            }
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(
                tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed,
            ) => break,
        }
    }
}

/// Maps a leaf's `Begun` to the switch stream's stage event.
fn stage_line(event: &ProgressEvent) -> String {
    format!("data: {}\n\n", serde_json::json!({ "stage": event.label }))
}

/// Maps the switch command's settled outcome to the stream's terminal event.
fn terminal_line(result: Result<commands::SharedOutcome, tokio::task::JoinError>) -> String {
    let payload = match result {
        Ok(outcome) => match &*outcome {
            Ok(profile) => serde_json::json!({ "status": "ready", "profile": profile }),
            #[cfg(feature = "local")]
            Err(GatewayError::PartialStart {
                profile,
                loaded,
                failed,
            }) => serde_json::json!({
                "status": "error",
                "profile": profile,
                "loaded": loaded,
                "failed": failed,
            }),
            Err(error) => serde_json::json!({
                "status": "error",
                "message": error_chain(error),
            }),
        },
        Err(join_error) => serde_json::json!({
            "status": "error",
            "message": format!("switch task failed: {join_error}"),
        }),
    };
    format!("data: {payload}\n\n")
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

fn config_path(state: &AppState) -> Result<&std::path::Path, GatewayError> {
    state
        .config
        .as_ref()
        .map(|config| config.path.as_path())
        .ok_or(GatewayError::ConfigPathUnavailable)
}

/// Compare the request's bearer token against the configured token.
///
/// The `/auth` browser handoff sets the key's session proof as an
/// HttpOnly cookie, so a request presenting that cookie authenticates
/// identically: the cookie is the key's ambient form, accepted anywhere
/// the bearer header is. Two guards shape the cookie path that the
/// bearer path does not need: the proof is recomputed from the
/// process-lifetime salt and the live key (the cookie never carries the
/// key itself), and the request must carry Fetch Metadata a cross-origin
/// page cannot strip, since an ambient credential would otherwise answer
/// to any same-site loopback page (ports are not part of a site).
pub(crate) async fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), GatewayError> {
    let presented = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    let live = state.live.read().await;
    if secret_eq(presented.as_bytes(), live.key.expose().as_bytes()) {
        return Ok(());
    }
    if let Some(cookie) = handoff::presented_cookie_proof(headers)
        && handoff::fetch_metadata_allows_cookie(headers)
        && secret_eq(
            &cookie,
            &handoff::session_token(&state.handoff_salt, live.key.expose().as_bytes()),
        )
    {
        return Ok(());
    }
    Err(GatewayError::Unauthorized)
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

#[cfg(all(test, feature = "stt"))]
mod transcription_auth_tests {
    #![expect(
        clippy::expect_used,
        reason = "the shared test fixture fails with the invariant named"
    )]

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_config::Config;
    use tower::ServiceExt;

    use crate::build_router;

    fn state() -> crate::AppState {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [workshop]\n",
        )
        .expect("config parses");
        app_state(config, None)
    }
    use crate::test_support::app_state;

    #[tokio::test]
    async fn transcription_checks_bearer_auth_before_multipart_extraction() {
        let response = build_router(state(), None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header("content-type", "not-multipart")
                    .body(Body::from("not multipart"))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "auth refuses the request before its malformed body is extracted"
        );
    }

    #[tokio::test]
    async fn unloaded_transcription_model_returns_openai_model_not_found() {
        const BOUNDARY: &str = "gateway-stt-boundary";
        let mut wav = vec![
            b'R', b'I', b'F', b'F', 36, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
            16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0, b'd', b'a',
            b't', b'a', 0, 0, 0, 0,
        ];
        let mut body = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nghost\r\n\
             --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\
             Content-Type: audio/wav\r\n\r\n"
        )
        .into_bytes();
        body.append(&mut wav);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        let response = build_router(state(), None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header("authorization", "Bearer test-token")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={BOUNDARY}"),
                    )
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
        assert_eq!(json["error"]["code"], "model_not_found");
    }
}

#[cfg(test)]
mod tray_status_tests {
    use gateway_config::Config;

    use crate::test_support::app_state;

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "3.5 + 1.0 is exact in binary floating point"
    )]
    fn the_tray_status_counts_routed_models_and_sums_declared_vram() {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
             [[model]]\nname = \"alpha\"\ndescription = \"a\"\ncontext = 1024\nupstream = \"a\"\nendpoints = [\"fake\"]\n\
             [[model]]\nname = \"beta\"\ndescription = \"b\"\ncontext = 1024\nupstream = \"b\"\nendpoints = [\"fake\"]\n\
             [[local_model]]\nname = \"gamma\"\ndescription = \"g\"\nsource = \"/models/gamma.gguf\"\ncontext = 4096\nvram_gb = 3.5\n\
             [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n",
        )
        .expect("config parses");
        let state = app_state(config, None);
        let (models, vram_gb) = state
            .tray_model_status()
            .expect("an uncontended state reads");
        assert_eq!(models, 2, "the harness routes the remote catalog");
        assert_eq!(vram_gb, 4.5, "local and STT declarations sum");
    }
}

#[cfg(test)]
mod provisioning_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use futures_util::future::BoxFuture;
    use gateway_config::{Config, ProfileName};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt as _;

    use crate::commands::{Command, Outcome};
    use crate::test_support::app_state;
    use crate::{AppState, build_router};

    /// A state whose catalog declares a local model the routing table never
    /// holds: `app_state` routes only the remote catalog, so `slow-model`
    /// stays configured-but-unloaded for the test's whole run.
    fn state() -> AppState {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [[local_model]]\nname = \"slow-model\"\ndescription = \"d\"\n\
             source = \"/models/slow.gguf\"\ncontext = 4096\n\
             [[profile]]\nname = \"main\"\nmodels = [\"slow-model\"]\n",
        )
        .expect("config parses");
        app_state(config, None)
    }

    /// An executor that parks every command until its token fires, then
    /// settles it as cancelled - the shape of a provisioning download.
    fn parking_executor() -> Arc<crate::commands::Executor> {
        Arc::new(|_state, command, _tree| {
            Box::pin(async move {
                match command {
                    Command::LoadProfile { name, token, .. } => {
                        token.cancelled().await;
                        Err(crate::error::GatewayError::CommandCancelled(format!(
                            "load-profile: {name}"
                        )))
                    }
                    _ => unreachable!("the test enqueues only LoadProfile"),
                }
            }) as BoxFuture<'static, Outcome>
        })
    }

    async fn chat(state: AppState, model: &str) -> axum::response::Response {
        build_router(state, None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"model":"{model}","messages":[{{"role":"user","content":"ping"}}]}}"#
                    )))
                    .expect("request builds"),
            )
            .await
            .expect("router answers")
    }

    #[tokio::test]
    async fn an_unloaded_but_configured_model_earns_a_503_naming_the_active_command() {
        let state = state();
        let worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");
        let _boot = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("main").expect("profile name"),
            false,
            CancellationToken::new(),
        ));
        tokio::time::timeout(Duration::from_secs(10), async {
            while state.commands.active_command().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the command goes active");

        let response = chat(state.clone(), "slow-model").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let text = std::str::from_utf8(&body).expect("the envelope is UTF-8");
        assert!(
            text.contains("model provisioning in progress"),
            "the 503 names the condition: {text}"
        );
        assert!(
            text.contains("load-profile: main"),
            "the 503 names the active command: {text}"
        );

        // A model the catalog does not name keeps its plain 404.
        let response = chat(state.clone(), "ghost").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        state.commands.cancel_active();
        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }

    /// A token fired after the start phase opens stops the switch before the
    /// persist and the routing-table swap. The canceller fires on the
    /// `starting-models` phase opening; the start itself is a no-op (the
    /// profile selects only a remote model), and the phase boundaries after
    /// the local start and after `replace_runtimes` keep the cancelled
    /// switch from committing.
    #[tokio::test]
    async fn a_token_fired_during_the_start_stops_the_persist_and_the_swap() {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\n\
             base_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
             [[model]]\nname = \"remote-model\"\ndescription = \"d\"\n\
             context = 8192\nupstream = \"u\"\nendpoints = [\"e\"]\n\
             [[profile]]\nname = \"main\"\nmodels = [\"remote-model\"]\n",
        )
        .expect("config parses");
        let state = app_state(config, None);
        let token = CancellationToken::new();
        let mut rx = state.hub.subscribe();
        let canceller = tokio::spawn({
            let token = token.clone();
            async move {
                while let Ok(event) = rx.recv().await {
                    if matches!(event.state, shared_progress::EventState::Begun { .. })
                        && event.label == "starting-models"
                    {
                        token.cancel();
                        return;
                    }
                }
            }
        });
        let tree = state.hub.operation();
        let outcome = crate::run_switch_with_config(
            state.clone(),
            ProfileName::parse("main").expect("profile name"),
            tree,
            None,
            || crate::StatePersistence::Write,
            &token,
        )
        .await;
        canceller.await.expect("the canceller ran");

        assert!(
            matches!(
                outcome,
                Err(crate::error::GatewayError::CommandCancelled(_))
            ),
            "the late cancellation stops the switch: {outcome:?}"
        );
        let live = state.live.read().await;
        assert!(
            live.profile_name.is_none(),
            "the cancelled switch never swapped the live state"
        );
    }
}

#[cfg(test)]
mod progress_tests {
    // Fractions are fixed-point millionths, so equality comparisons are exact.
    #![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::StreamExt as _;
    use shared_progress::{EventState, ProgressEvent, ProgressHub};

    use super::{PROGRESS_HEARTBEAT, progress_sse_response};

    const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

    /// Reads `data:` payloads from the body until `count` events arrive,
    /// skipping heartbeat comments. Frames may split or coalesce SSE events,
    /// so the text accumulates across reads.
    async fn read_events<S>(frames: &mut S, count: usize) -> Vec<ProgressEvent>
    where
        S: futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
    {
        let mut text = String::new();
        let mut events = Vec::new();
        while events.len() < count {
            let frame = tokio::time::timeout(FRAME_TIMEOUT, frames.next())
                .await
                .expect("the progress stream stalled")
                .expect("the progress stream ended early")
                .expect("the progress stream errored");
            let chunk = std::str::from_utf8(&frame).expect("SSE frames are UTF-8");
            text.push_str(chunk);
            while let Some(end) = text.find("\n\n") {
                let block: String = text.drain(..end + 2).collect();
                if let Some(data) = block.trim().strip_prefix("data: ") {
                    events
                        .push(serde_json::from_str(data).expect("a data line is a ProgressEvent"));
                }
            }
        }
        events
    }

    /// Reads `data:` payloads until `stop` matches one, returning everything
    /// read, the matching event last.
    async fn read_until<S>(
        frames: &mut S,
        stop: impl Fn(&ProgressEvent) -> bool,
    ) -> Vec<ProgressEvent>
    where
        S: futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
    {
        let mut text = String::new();
        let mut events = Vec::new();
        loop {
            let frame = tokio::time::timeout(FRAME_TIMEOUT, frames.next())
                .await
                .expect("the progress stream stalled")
                .expect("the progress stream ended early")
                .expect("the progress stream errored");
            let chunk = std::str::from_utf8(&frame).expect("SSE frames are UTF-8");
            text.push_str(chunk);
            while let Some(end) = text.find("\n\n") {
                let block: String = text.drain(..end + 2).collect();
                if let Some(data) = block.trim().strip_prefix("data: ") {
                    let event: ProgressEvent =
                        serde_json::from_str(data).expect("a data line is a ProgressEvent");
                    let done = stop(&event);
                    events.push(event);
                    if done {
                        return events;
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn the_stream_carries_begun_updated_finished_in_order() {
        let hub = Arc::new(ProgressHub::new());
        let response = progress_sse_response(&hub);
        let mut frames = response.into_body().into_data_stream();

        let tree = hub.operation();
        let leaf = tree.register("download", 1.0);
        leaf.set_fraction(0.5);
        leaf.complete();

        let events = read_events(&mut frames, 3).await;
        assert!(matches!(events[0].state, EventState::Begun { weight } if weight == 1.0));
        assert!(
            matches!(events[1].state, EventState::Updated { fraction } if fraction == 0.5),
            "the intermediate sample follows Begun: {:?}",
            events[1]
        );
        assert!(matches!(events[2].state, EventState::Finished { ok: true }));
        assert!(events.iter().all(|event| event.path == "download"));
    }

    #[tokio::test]
    async fn a_fresh_subscriber_first_receives_a_snapshot_of_live_operations() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("download", 1.0);
        leaf.set_fraction(0.5);

        // The subscriber connects after the work began, so the broadcast
        // alone would show nothing until the next report: the snapshot must
        // carry the current state.
        let response = progress_sse_response(&hub);
        let mut frames = response.into_body().into_data_stream();
        let events = read_events(&mut frames, 2).await;
        assert!(matches!(events[0].state, EventState::Begun { weight } if weight == 1.0));
        assert!(matches!(events[1].state, EventState::Updated { fraction } if fraction == 0.5));
        assert_eq!(events[0].operation, tree.operation());
    }

    #[tokio::test]
    async fn a_fresh_subscriber_sees_a_finished_leafs_terminal_state() {
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("download", 1.0);
        leaf.set_fraction(0.5);
        leaf.fail();

        // The leaf finished before the subscriber connected; without a
        // replayed Finished the subscriber would hold it as unfinished until
        // the tree detaches.
        let response = progress_sse_response(&hub);
        let mut frames = response.into_body().into_data_stream();
        let events = read_events(&mut frames, 3).await;
        assert!(matches!(events[0].state, EventState::Begun { .. }));
        assert!(
            matches!(events[1].state, EventState::Updated { fraction } if fraction == 0.5),
            "a failed leaf keeps its fraction: {:?}",
            events[1]
        );
        assert!(
            matches!(events[2].state, EventState::Finished { ok: false }),
            "the terminal state replays: {:?}",
            events[2]
        );
    }

    #[tokio::test]
    async fn a_lagged_subscriber_drops_the_overflow_and_carries_on() {
        let hub = Arc::new(ProgressHub::new());
        let response = progress_sse_response(&hub);
        let mut frames = response.into_body().into_data_stream();

        let tree = hub.operation();
        // Overflow the hub's 1024-event broadcast ring before the stream's
        // first poll, so its receiver lags: the Lagged arm must drop the
        // skipped events and continue rather than ending the stream.
        let _leaves: Vec<_> = (0..1100)
            .map(|index| tree.register(&format!("leaf-{index}"), 1.0))
            .collect();
        let last = tree.register("last", 1.0);
        last.complete();

        let events = read_until(&mut frames, |event| {
            event.path == "last" && matches!(event.state, EventState::Finished { ok: true })
        })
        .await;
        assert!(
            events.len() <= 1024,
            "the overflowed prefix is dropped, not delivered: {} events",
            events.len()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_hub_emits_heartbeat_comments_on_cadence() {
        let hub = Arc::new(ProgressHub::new());
        let response = progress_sse_response(&hub);
        let mut frames = response.into_body().into_data_stream();

        // No operations are live, so heartbeat comments are the only
        // traffic; two ticks pin the cadence, not just the first deadline.
        for _ in 0..2 {
            let frame = tokio::time::timeout(PROGRESS_HEARTBEAT + FRAME_TIMEOUT, frames.next())
                .await
                .expect("the progress stream stalled")
                .expect("the progress stream ended early")
                .expect("the progress stream errored");
            assert_eq!(
                std::str::from_utf8(&frame).expect("SSE frames are UTF-8"),
                ": heartbeat\n\n"
            );
        }
    }

    #[tokio::test]
    async fn the_stream_goes_quiet_when_the_tree_drops() {
        let hub = Arc::new(ProgressHub::new());
        let response = progress_sse_response(&hub);
        let mut frames = response.into_body().into_data_stream();

        let tree = hub.operation();
        let _leaf = tree.register("download", 1.0);
        let events = read_events(&mut frames, 1).await;
        assert!(matches!(events[0].state, EventState::Begun { .. }));

        drop(tree);
        // The first heartbeat is 15 s out, so nothing may arrive inside this
        // window: a detached tree emits no events and an idle hub is silent.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), frames.next())
                .await
                .is_err(),
            "a dropped tree must leave the stream quiet until the next heartbeat"
        );
    }
}

#[cfg(test)]
mod switch_tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use futures_util::StreamExt as _;
    use shared_progress::ProgressHub;

    use super::{GatewayError, drain_switch_stages, switch_sse_response};

    /// Collects the finite switch stream's full body text.
    async fn body_text(response: axum::response::Response) -> String {
        let mut frames = response.into_body().into_data_stream();
        let mut text = String::new();
        while let Some(frame) = frames.next().await {
            let frame = frame.expect("the switch stream errored");
            text.push_str(std::str::from_utf8(&frame).expect("SSE frames are UTF-8"));
        }
        text
    }

    #[tokio::test]
    async fn the_switch_stream_shows_only_its_own_operations_stages() {
        let hub = Arc::new(ProgressHub::new());
        let rx = hub.subscribe();
        let tree = hub.operation();
        let operation = tree.operation();
        let other = hub.operation();
        let _unrelated = other.register("download", 1.0);
        let _leaf = tree.register("loading-profile", 1.0);
        let switch = tokio::spawn(async { Arc::new(Ok::<_, GatewayError>("beta".to_owned())) });

        let text = body_text(switch_sse_response(rx, operation, switch)).await;
        assert!(
            text.contains("\"stage\":\"loading-profile\""),
            "body: {text}"
        );
        assert!(
            !text.contains("download"),
            "another operation's leaf must not leak into the switch stream: {text}"
        );
        assert!(
            text.contains("\"status\":\"ready\"") && text.contains("\"profile\":\"beta\""),
            "the terminal event comes from the join result: {text}"
        );
    }

    #[tokio::test]
    async fn the_switch_stream_ends_with_the_terminal_error_from_the_join_result() {
        let hub = Arc::new(ProgressHub::new());
        let rx = hub.subscribe();
        let tree = hub.operation();
        let operation = tree.operation();
        let switch = tokio::spawn(async {
            Arc::new(Err::<String, _>(GatewayError::ProfileNotFound(
                "ghost".to_owned(),
            )))
        });

        let text = body_text(switch_sse_response(rx, operation, switch)).await;
        assert!(text.contains("\"status\":\"error\""), "body: {text}");
        assert!(text.contains("profile not found: ghost"), "body: {text}");
    }

    #[tokio::test]
    #[cfg(feature = "local")]
    async fn partial_start_terminal_reports_every_ready_and_failed_model() {
        let hub = Arc::new(ProgressHub::new());
        let rx = hub.subscribe();
        let tree = hub.operation();
        let operation = tree.operation();
        let switch = tokio::spawn(async {
            Arc::new(Err::<String, _>(GatewayError::PartialStart {
                profile: "beta".to_owned(),
                loaded: vec!["ready".to_owned()],
                failed: vec!["broken: startup error".to_owned()],
            }))
        });

        let text = body_text(switch_sse_response(rx, operation, switch)).await;

        assert!(text.contains("\"profile\":\"beta\""), "body: {text}");
        assert!(text.contains("\"loaded\":[\"ready\"]"), "body: {text}");
        assert!(
            text.contains("\"failed\":[\"broken: startup error\"]"),
            "body: {text}"
        );
    }

    #[tokio::test]
    async fn the_switch_stream_survives_broadcast_lag() {
        let hub = Arc::new(ProgressHub::new());
        let rx = hub.subscribe();
        let tree = hub.operation();
        let operation = tree.operation();

        // Overflow the hub's 1024-event ring before the stream's first poll,
        // so the receiver lags: the Lagged arm must drop the skipped events
        // and carry on rather than ending the stream.
        let noise = hub.operation();
        let _noise_leaves: Vec<_> = (0..1100)
            .map(|index| noise.register(&format!("noise-{index}"), 1.0))
            .collect();
        let _leaf = tree.register("loading-profile", 1.0);
        let switch = tokio::spawn(async { Arc::new(Ok::<_, GatewayError>("beta".to_owned())) });

        let text = body_text(switch_sse_response(rx, operation, switch)).await;
        assert!(
            text.contains("\"stage\":\"loading-profile\""),
            "the stage event survives the lag: {text}"
        );
        assert!(
            text.contains("\"status\":\"ready\"") && text.contains("\"profile\":\"beta\""),
            "the terminal event comes from the join result: {text}"
        );
    }

    #[test]
    fn completed_switch_catch_up_continues_after_a_lag_marker() {
        let hub = Arc::new(ProgressHub::new());
        let mut rx = hub.subscribe();
        let noise = hub.operation();
        let _noise_leaves: Vec<_> = (0..1100)
            .map(|index| noise.register(&format!("noise-{index}"), 1.0))
            .collect();
        let tree = hub.operation();
        let operation = tree.operation();
        let _loading = tree.register("loading-profile", 1.0);
        let mut pending = VecDeque::new();

        drain_switch_stages(&mut rx, operation, &mut pending);

        assert_eq!(
            pending,
            [r#"data: {"stage":"loading-profile"}

"#]
        );
    }
}

#[cfg(test)]
mod loopback_wall_tests {
    //! The shared loopback wall over the admin config surface: every
    //! walled path refuses a LAN peer with 403 even when it presents the
    //! valid bearer key, admits a loopback peer past the wall, and fails
    //! closed when no peer address exists; the bearer-only routes stay
    //! reachable from any source. The `config-ui` feature's `/config`
    //! mount and redirect are pinned here too, in both feature states.

    use std::net::SocketAddr;

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{Method, Request, Response, StatusCode};
    use gateway_config::Config;
    use tower::ServiceExt;

    use crate::test_support::{AdminPaths, app_state};
    use crate::{AppState, build_router};

    /// A tempdir-backed state with real profiles and boot files, so every
    /// walled handler has something to answer with once past the wall.
    fn fixture() -> (tempfile::TempDir, AppState) {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let models = temp.path().join("cache").join("models");
        std::fs::create_dir_all(&models).expect("mkdir cache models");
        let boot = temp.path().join("gateway.toml");
        std::fs::write(&boot, "").expect("write boot");
        let config = Config::from_toml_str(&format!(
            r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = '{cache}'
"#,
            cache = temp.path().join("cache").display(),
        ))
        .expect("the fixture profile parses");
        let state = app_state(
            config,
            Some(AdminPaths {
                fixture_dir: temp.path().to_path_buf(),
                active: "main".to_owned(),
                config_path: boot,
            }),
        );
        (temp, state)
    }

    /// Every admin config path behind the shared wall, with the method
    /// exercised against it. The HF requests are deliberately malformed
    /// (a duplicate query key, a slashless repo) so a loopback sweep is
    /// refused at validation and never reaches the real hub; every other
    /// empty-bodied write fails its own extractor the same way. All of
    /// that happens past the wall, so any non-403 status proves
    /// admission.
    fn walled_requests() -> Vec<(Method, &'static str)> {
        let mut requests = vec![
            (Method::GET, "/admin/config"),
            (Method::PUT, "/admin/config"),
            (Method::GET, "/admin/env"),
            (Method::PUT, "/admin/env"),
            (Method::GET, "/admin/config-pending"),
            (Method::GET, "/admin/config-dirty"),
            (Method::POST, "/admin/config-apply"),
            (Method::POST, "/admin/config-revert"),
            (Method::GET, "/admin/system"),
            (Method::GET, "/admin/hf/search?q=a&q=b"),
            (Method::GET, "/admin/hf/model/owner/na%20me"),
            (Method::POST, "/admin/reveal"),
        ];
        #[cfg(feature = "local")]
        requests.extend([
            (Method::GET, "/admin/chat-templates"),
            (Method::GET, "/admin/orphans"),
            (Method::GET, "/admin/model-info"),
        ]);
        requests
    }

    /// Sends one empty-bodied request through `build_router` with the
    /// valid bearer key and the given peer address planted as the
    /// `ConnectInfo` extension (or none at all).
    async fn send_with_peer(
        state: AppState,
        method: Method,
        path: &str,
        peer: Option<&str>,
    ) -> Response<Body> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(AUTHORIZATION, "Bearer test-token")
            .body(Body::empty())
            .expect("static request parts are valid");
        if let Some(peer) = peer {
            let peer: SocketAddr = peer.parse().expect("a socket address");
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        build_router(state, None)
            .oneshot(request)
            .await
            .expect("the router is infallible")
    }

    #[tokio::test]
    async fn every_walled_path_refuses_a_lan_peer_with_403() {
        let (_temp, state) = fixture();
        for (method, path) in walled_requests() {
            let status = send_with_peer(
                state.clone(),
                method.clone(),
                path,
                Some("198.51.100.7:44821"),
            )
            .await
            .status();
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} must refuse a LAN peer even with the valid bearer key"
            );
        }
    }

    #[tokio::test]
    async fn every_walled_path_admits_a_loopback_peer_past_the_wall() {
        let (_temp, state) = fixture();
        for (method, path) in walled_requests() {
            let status =
                send_with_peer(state.clone(), method.clone(), path, Some("127.0.0.1:50000"))
                    .await
                    .status();
            assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} must pass the wall for a loopback peer"
            );
        }
    }

    #[tokio::test]
    async fn every_walled_path_fails_closed_without_a_peer_address() {
        let (_temp, state) = fixture();
        for (method, path) in walled_requests() {
            let status = send_with_peer(state.clone(), method.clone(), path, None)
                .await
                .status();
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} must fail closed when the peer address is unknown"
            );
        }
    }

    #[tokio::test]
    async fn the_bearer_only_routes_stay_reachable_from_the_lan() {
        let (_temp, state) = fixture();
        for path in [
            "/admin/status",
            "/admin/profiles",
            "/admin/progress",
            "/v1/models",
        ] {
            let status =
                send_with_peer(state.clone(), Method::GET, path, Some("198.51.100.7:44821"))
                    .await
                    .status();
            assert_eq!(
                status,
                StatusCode::OK,
                "GET {path} keeps its bearer-only, any-source behavior"
            );
        }
        // The switch route stays any-source too; the empty body fails its
        // own extractor past auth, so any non-403 status proves the wall
        // is absent (the same trick as the loopback-admission sweep).
        let status = send_with_peer(
            state,
            Method::POST,
            "/admin/switch-profile",
            Some("198.51.100.7:44821"),
        )
        .await
        .status();
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "POST /admin/switch-profile keeps its bearer-only, any-source behavior"
        );
    }

    #[tokio::test]
    async fn admin_status_reports_a_stable_config_generation() {
        let (_temp, state) = fixture();
        let first = send_with_peer(
            state.clone(),
            Method::GET,
            "/admin/status",
            Some("127.0.0.1:50000"),
        )
        .await;
        let second =
            send_with_peer(state, Method::GET, "/admin/status", Some("127.0.0.1:50000")).await;
        let first: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(first.into_body(), usize::MAX)
                .await
                .expect("read first status"),
        )
        .expect("parse first status");
        let second: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(second.into_body(), usize::MAX)
                .await
                .expect("read second status"),
        )
        .expect("parse second status");
        let generation = first["config_generation"]
            .as_str()
            .expect("status generation is a string");
        assert!(
            !generation.is_empty(),
            "the generation identifies this process"
        );
        assert_eq!(
            generation,
            second["config_generation"]
                .as_str()
                .expect("second status generation is a string"),
            "one process reports one stable generation"
        );
    }

    #[cfg(feature = "config-ui")]
    #[tokio::test]
    async fn config_without_a_trailing_slash_redirects_to_the_mount() {
        let (_temp, state) = fixture();
        let response = send_with_peer(state, Method::GET, "/config", Some("127.0.0.1:50000")).await;
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .expect("a redirect carries a Location header"),
            "/config/",
            "the redirect lands on the trailing-slash mount so relative asset paths resolve"
        );
    }

    #[cfg(feature = "config-ui")]
    #[tokio::test]
    async fn the_config_ui_is_served_at_the_trailing_slash_mount() {
        let (_temp, state) = fixture();
        for path in ["/config/", "/config/app.js"] {
            let status = send_with_peer(state.clone(), Method::GET, path, Some("127.0.0.1:50000"))
                .await
                .status();
            assert_eq!(status, StatusCode::OK, "GET {path} serves the SPA asset");
        }
    }

    #[cfg(feature = "config-ui")]
    #[tokio::test]
    async fn the_config_surface_refuses_a_lan_peer() {
        let (_temp, state) = fixture();
        for path in ["/config", "/config/", "/config/app.js"] {
            let status =
                send_with_peer(state.clone(), Method::GET, path, Some("198.51.100.7:44821"))
                    .await
                    .status();
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "GET {path} must refuse a LAN peer"
            );
        }
    }

    #[cfg(not(feature = "config-ui"))]
    #[tokio::test]
    async fn without_the_feature_no_config_routes_exist() {
        let (_temp, state) = fixture();
        for path in [
            "/config",
            "/config/",
            "/config/app.js",
            "/auth?key=test-token",
        ] {
            let status = send_with_peer(state.clone(), Method::GET, path, Some("127.0.0.1:50000"))
                .await
                .status();
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "GET {path} must not exist in a build without the config-ui feature"
            );
        }
    }

    /// Sends one request through `build_router` with the host wall
    /// installed for `bound`, with the given `Host` header (or none).
    async fn send_with_host(state: AppState, path: &str, host: Option<&str>) -> StatusCode {
        let bound: SocketAddr = "127.0.0.1:8081".parse().expect("a socket address");
        let mut builder = Request::builder()
            .uri(path)
            .header(AUTHORIZATION, "Bearer test-token");
        if let Some(host) = host {
            builder = builder.header(axum::http::header::HOST, host);
        }
        build_router(state, Some(bound))
            .oneshot(
                builder
                    .body(Body::empty())
                    .expect("static request parts are valid"),
            )
            .await
            .expect("the router is infallible")
            .status()
    }

    #[tokio::test]
    async fn the_host_wall_refuses_a_foreign_host_on_every_route() {
        let (_temp, state) = fixture();
        // `/health` is deliberately not exempt: the connection-file probe
        // sends the bound address as Host, so the wall keeps it honest.
        for path in ["/health", "/admin/status", "/v1/models", "/shutdown"] {
            assert_eq!(
                send_with_host(state.clone(), path, Some("attacker.com")).await,
                StatusCode::FORBIDDEN,
                "{path} must refuse a rebound hostname"
            );
        }
    }

    #[tokio::test]
    async fn the_host_wall_admits_the_bound_and_localhost_authorities() {
        let (_temp, state) = fixture();
        for host in ["127.0.0.1:8081", "localhost:8081"] {
            assert_eq!(
                send_with_host(state.clone(), "/health", Some(host)).await,
                StatusCode::OK,
                "Host: {host} names the bound socket"
            );
        }
        // A missing authority fails closed.
        assert_eq!(
            send_with_host(state.clone(), "/health", None).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn the_router_seam_without_a_bound_address_carries_no_host_wall() {
        let (_temp, state) = fixture();
        let response = build_router(state, None)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header(axum::http::header::HOST, "attacker.com")
                    .body(Body::empty())
                    .expect("static request parts are valid"),
            )
            .await
            .expect("the router is infallible");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "the `Gateway::router` seam has no bound socket to allowlist"
        );
    }
}
