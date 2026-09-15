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
//! catalog with `POST /admin/switch-profile` persisting the selection and
//! reporting `restart_required`, a bearer-authed `GET /admin/status` readout
//! carrying the command queue's active and pending commands plus one
//! readiness entry per capability endpoint, bearer-authed
//! `POST /admin/queue/cancel` and `POST /admin/queue/cancel-pending`
//! cancelling the queue's active and waiting commands, a bearer-authed
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
//! (stt builds), a bearer-authed `POST /v1/audio/speech` speech-synthesis
//! passthrough for `kind = "speech"` models streaming the upstream's audio
//! bytes unread, a bearer-authed `GET /v1/audio/voices` union catalog of
//! the speech models' configured voices, a bearer-authed
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
//! bearer-authed `GET /admin/cloud-models` readout of the cached cloud
//! provider model sheet (with `POST /admin/cloud-models/refresh` forcing
//! a re-download and answering with the fresh sheet), a loopback-only, bearer-authed
//! `POST /shutdown` driving the same
//! graceful shutdown Ctrl-C drives - and
//! `GET /health`. The whole admin config surface (config read/write, env,
//! pending state, apply/revert, orphans, system, model-info, the HF
//! proxy, cloud-models, reveal, shutdown) sits behind the shared loopback
//! wall from `shared-loopback` in every build; with the
//! default-on `stt` feature, `WS /v1/realtime?intent=transcription`
//! serves Gateway-owned Realtime transcription beside the batch route;
//! with the
//! `config-ui` feature the embedded config SPA is served at `/config/`
//! behind the same wall, and `GET /auth?key=` sets a session proof
//! derived from the bearer key as an HttpOnly cookie and redirects to the
//! key-free `/config/`, so a browser handoff never leaves the key in
//! browser history. With `[server] trust_loopback` on (the default), a
//! loopback peer presenting no credential is admitted to every route
//! unless its Fetch Metadata marks a cross-origin page; `trust_loopback =
//! false` requires the bearer key from every caller. When the listener is
//! bound to loopback, every route additionally sits behind the shared
//! host-authority wall, which refuses requests whose `Host` is not the
//! bound socket (the DNS-rebinding defense). In-process
//! llama.cpp FFI and endpoint pinning are deferred.

mod api_error;
mod auth;
mod boot;
mod boot_load;
#[cfg(feature = "local")]
mod cache;
#[cfg(feature = "local")]
mod chat_templates;
mod cloud_models;
mod commands;
mod config_apply;
mod config_pending;
mod config_write;
mod diagnostics;
mod dialect;
mod env_file;
mod error;
mod handoff;
mod hf;
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
pub(crate) use gateway_protocol::{upstream, wire};
// The dominion admission queues live in the routing crate; this re-export
// keeps every `crate::queue::*` path resolving unchanged.
pub(crate) use gateway_routing::queue;
// Local inference lives in its own crate behind the `local` feature; this
// re-export keeps every `crate::local::*` path resolving unchanged.
#[cfg(feature = "local")]
pub(crate) use gateway_local as local;

pub use crate::api_error::{ServeError, StartupError, StartupErrorKind};
#[cfg(not(feature = "local"))]
pub(crate) use crate::boot_load::LOCAL_MODELS_UNSUPPORTED;
#[cfg(not(feature = "stt"))]
pub(crate) use crate::boot_load::STT_RUNTIME_UNAVAILABLE;
pub use crate::diagnostics::diagnostics_json;
pub use crate::relaunch::{GatewayStartup, GatewayStartupError, settle_gateway_startup};
pub use crate::runner::{
    Gateway, GatewayHandle, ProfilesContext, ServeOptions, run, run_printing_url, spawn,
};
pub use crate::tray::run_with_tray;
pub use gateway_config::{
    Config, ConfigError, ConfigErrorKind, ProfileName, ProfileNameError, Secret,
};

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Request, State};
use axum::http::HeaderValue;
#[cfg(feature = "stt")]
use axum::http::header::ORIGIN;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::response::Response;
#[cfg(feature = "local")]
use axum::routing::delete;
use axum::routing::{get, post};
use axum::{Router, response::IntoResponse};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::auth::Caller;
use crate::error::GatewayError;
#[cfg(feature = "local")]
use crate::local::LocalRuntime;
use crate::routing::Routing;
use crate::wire::{
    ChatRequest, EmbeddingRequest, EmbeddingResponse, ModelInfo, RerankRequest, RerankResponse,
    SpeechRequest, SpeechResponseFormat, SpeechStreamFormat, SpeechVoice,
};
use gateway_config::ModelKind;
#[cfg(feature = "web-search")]
use gateway_config::WebSearchConfig;
use gateway_protocol::ProtocolError;
#[cfg(feature = "stt")]
use gateway_stt::SpeechService;
#[cfg(feature = "web-search")]
use gateway_web_search::{WebSearchRequest, WebSearchResponse, WebSearchState};
use shared_progress::{EventState, ProgressEvent, ProgressHub};

/// Mutable live configuration held behind a lock so the boot load and a
/// config apply can swap routing without rebuilding the axum router.
#[derive(Debug)]
struct LiveState {
    routing: Arc<Routing>,
    key: Secret,
    /// Whether a loopback peer presenting no credential is admitted
    /// (`[server] trust_loopback`). Read from the boot config at assembly;
    /// `[server]` is process-owned, so a change takes effect on restart.
    trust_loopback: bool,
    /// The running configuration, retained so `GET /admin/config` can render
    /// it; swapped (with the routing table) by a config apply, which
    /// publishes the applied document with no profile selected;
    /// `profile_name` alone names the running profile.
    config: Arc<Config>,
    /// The declared VRAM of the speech-to-text models the boot selection
    /// loaded, summed at assembly. The speech runtime is fixed for the
    /// process lifetime while `config` is swapped by every apply, so the
    /// status readout keeps its own copy.
    stt_vram_gb: f64,
    #[cfg(feature = "web-search")]
    web_search: Option<Arc<WebSearchState>>,
    #[cfg(feature = "local")]
    local: LocalRuntime,
    profile_name: Option<String>,
    /// The active profile's `models` allowlist, when it declared one.
    model_allowlist: Option<Vec<String>>,
    /// Local models of the boot profile whose children are spawning.
    /// Published by the boot load once their artifacts are staged and
    /// cleared by its commit or its failure, so a request for one of them
    /// earns [`GatewayError::ModelLoading`] instead of a 404 while the
    /// spawn runs, and never afterwards.
    loading: BTreeSet<String>,
}

impl LiveState {
    /// The number of models in the live routing table and the declared VRAM
    /// total of the running local children and the boot STT selection, for
    /// the tray's status line and `GET /admin/status`.
    ///
    /// The local total is derived from the children the runtime holds,
    /// looked up by name in the catalog for their declaration, not from the
    /// config's selected subset: an apply swaps `config` for a document
    /// parsed with no selection while the children keep running.
    fn model_status(&self) -> (usize, f64) {
        let models = self.routing.models().len();
        #[cfg(feature = "local")]
        let local_vram_gb = {
            let declared = self.config.catalog_local_models();
            self.local
                .models()
                .iter()
                .filter_map(|running| {
                    declared
                        .iter()
                        .find(|model| model.name() == running.name)
                        .and_then(gateway_config::LocalModelConfig::vram_gb)
                })
                .sum::<f64>()
        };
        #[cfg(not(feature = "local"))]
        let local_vram_gb = 0.0;
        (models, local_vram_gb + self.stt_vram_gb)
    }
}

/// Single configuration file used by admin routes and profile persistence.
#[derive(Debug)]
struct AdminConfig {
    path: std::path::PathBuf,
}

/// What the active profile selected: its name and its `models` allowlist.
/// Both are reported by `GET /admin/status` and fixed at assembly for the
/// process lifetime.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProfileSelection {
    /// The active profile name.
    pub(crate) name: Option<String>,
    /// The active profile's `models` allowlist, when it declared one.
    pub(crate) model_allowlist: Option<Vec<String>>,
}

/// Shared handler state: live routing/key/local runtime, configuration path,
/// and command coordination.
#[derive(Debug, Clone)]
pub(crate) struct AppState {
    live: Arc<RwLock<LiveState>>,
    config: Option<Arc<AdminConfig>>,
    /// Process-lifetime identifier used by the config UI to detect a restart.
    config_generation: Arc<str>,
    /// Protects shadow-file consistency: the census-and-capture step of
    /// `POST /admin/config-apply`, the Apply command's commit,
    /// `POST /admin/config-revert`, every shadow-writing `PUT` save, and
    /// the switch route's state write serialize on it, so Apply only
    /// captures shadow combinations the latest save validated whole, never
    /// half-promotes one, and no pending read observes a half-written
    /// selection. Held for those short steps only, never across a download.
    apply: Arc<tokio::sync::Mutex<()>>,
    /// The process-lifetime progress broker: operations attach trees for
    /// their own lifetimes, and `GET /admin/progress` streams its events.
    hub: Arc<ProgressHub>,
    /// The command queue: the boot load, config applies, and unloads run
    /// as serialized, cancellable commands; the tray and routes read its
    /// status in-process.
    commands: commands::CommandQueue,
    /// Shared host-metrics sampler for `GET /admin/system`: one process-wide
    /// `sysinfo::System` so CPU-utilization deltas span requests, plus the
    /// once-per-process NVML probe.
    metrics: Arc<std::sync::Mutex<system::SystemSampler>>,
    /// Shared Hugging Face hub client for the `GET /admin/hf/*` proxy
    /// routes: one reqwest client plus the boot-time `HF_TOKEN`.
    hf: Arc<hf::HfProxy>,
    /// The cloud provider model sheet cache behind
    /// `GET /admin/cloud-models`: loaded from the profile directory at
    /// launch and refreshed by one bounded background download at a time.
    cloud_models: cloud_models::CloudModels,
    /// Launches the OS file manager for `POST /admin/reveal`; injectable
    /// so tests assert the constructed command without spawning anything.
    reveal: Arc<dyn reveal::RevealLauncher>,
    /// The process-shutdown signal fired by `POST /shutdown`; the serve
    /// loop selects on it alongside the caller-owned shutdown future.
    shutdown: shutdown::ShutdownSignal,
    /// Process-lifetime random salt for the `/auth` handoff's session
    /// proof; a restart or key rotation invalidates every minted cookie.
    handoff_salt: [u8; 32],
    /// Process-lifetime speech facade shared by routes and the boot load.
    #[cfg(feature = "stt")]
    speech: SpeechService,
    /// Test-only rendezvous a command awaits at the start of one named
    /// phase, so a test can hold the boot load inside its download or its
    /// spawn, or an apply before its commit, and observe the live state
    /// there. `None` in production and in every test that does not
    /// install one.
    #[cfg(test)]
    park: Option<Arc<park::PhasePark>>,
}

/// The test-only phase rendezvous for the boot load and the apply command.
#[cfg(test)]
pub(crate) mod park {
    use tokio::sync::Notify;

    /// One command phase a test can park.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Phase {
        /// The boot load's artifact download, before anything is promised
        /// as loading.
        Download,
        /// The boot load's child spawn, once the local models are promised
        /// as loading.
        Spawn,
        /// A config apply's commit, before it takes the apply lock: the
        /// captured shadows are not yet promoted and nothing is live.
        ApplyCommit,
    }

    /// Parks the command at `phase` until the test releases it. Single use:
    /// each notify stores one permit, so a release before the command
    /// arrives is not lost.
    #[derive(Debug)]
    pub(crate) struct PhasePark {
        phase: Phase,
        entered: Notify,
        release: Notify,
    }

    impl PhasePark {
        pub(crate) fn at(phase: Phase) -> PhasePark {
            PhasePark {
                phase,
                entered: Notify::new(),
                release: Notify::new(),
            }
        }

        /// Resolves once the command has parked at the phase.
        pub(crate) async fn entered(&self) {
            self.entered.notified().await;
        }

        /// Lets the parked command continue.
        pub(crate) fn release(&self) {
            self.release.notify_one();
        }

        pub(crate) async fn park(&self, phase: Phase) {
            if phase == self.phase {
                self.entered.notify_one();
                self.release.notified().await;
            }
        }
    }
}

impl AppState {
    /// Awaits the installed test rendezvous at `phase`; a no-op in
    /// production and without one installed.
    #[cfg(test)]
    async fn park_at(&self, phase: park::Phase) {
        if let Some(park) = &self.park {
            park.park(phase).await;
        }
    }

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
        #[cfg(feature = "stt")] speech: SpeechService,
        #[cfg(feature = "web-search")] web_search: Option<&WebSearchConfig>,
        config_path: Option<std::path::PathBuf>,
        selection: ProfileSelection,
        hub: Arc<ProgressHub>,
    ) -> AppState {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        AppState {
            live: Arc::new(RwLock::new(LiveState {
                routing,
                key,
                trust_loopback: config.server().trust_loopback(),
                stt_vram_gb: config
                    .stt_models()
                    .iter()
                    .map(gateway_config::SttModelConfig::vram_gb)
                    .sum(),
                config,
                #[cfg(feature = "web-search")]
                web_search: web_search.map(|cfg| Arc::new(WebSearchState::new(cfg))),
                #[cfg(feature = "local")]
                local,
                profile_name: selection.name,
                model_allowlist: selection.model_allowlist,
                loading: BTreeSet::new(),
            })),
            config: config_path.map(|path| Arc::new(AdminConfig { path })),
            config_generation: format!("{}-{started}", std::process::id()).into(),
            apply: Arc::new(tokio::sync::Mutex::new(())),
            commands: commands::CommandQueue::new(Arc::clone(&hub)),
            hub,
            metrics: Arc::new(std::sync::Mutex::new(system::SystemSampler::new())),
            hf: Arc::new(hf::HfProxy::from_env()),
            cloud_models: cloud_models::CloudModels::default(),
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
            speech,
            #[cfg(test)]
            park: None,
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

    /// A point-in-time readout for the tray's status line: the number of
    /// models in the live routing table and the declared VRAM total of the
    /// active local and STT models.
    ///
    /// Returns `None` when a command holds the live-state write lock: the
    /// tray's timer skips that tick rather than blocking the message loop.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux", test))]
    pub(crate) fn tray_model_status(&self) -> Option<(usize, f64)> {
        let live = self.live.try_read().ok()?;
        Some(live.model_status())
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
        .route("/v1/audio/speech", post(audio_speech))
        .route("/v1/audio/voices", get(audio_voices))
        .route("/v1/models", get(list_models))
        .route("/health", get(health))
        .route("/admin/profiles", get(admin_list_profiles))
        .route("/admin/status", get(admin_status))
        .route("/admin/progress", get(admin_progress))
        .route("/admin/switch-profile", post(admin_switch_profile))
        .route("/admin/queue/cancel", post(admin_queue_cancel))
        .route(
            "/admin/queue/cancel-pending",
            post(admin_queue_cancel_pending),
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
        .route("/admin/cloud-models", get(cloud_models::admin_cloud_models))
        .route(
            "/admin/cloud-models/refresh",
            post(cloud_models::admin_cloud_models_refresh),
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
    #[cfg(feature = "stt")]
    let speech_routes = state.speech.routes();
    let router = router.with_state(state.clone());
    #[cfg(feature = "stt")]
    let router = router.merge(
        speech_routes.route_layer(axum::middleware::from_fn_with_state(
            state,
            authorize_stt_route,
        )),
    );
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
    caller: Caller,
    Json(request): Json<WebSearchRequest>,
) -> Result<Json<WebSearchResponse>, GatewayError> {
    check_auth(&state, &caller).await?;
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

#[cfg(feature = "stt")]
async fn authorize_stt_route(
    State(state): State<AppState>,
    caller: Caller,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<Response, GatewayError> {
    check_auth(&state, &caller).await?;
    if request.uri().path() == "/v1/realtime" && !gateway_realtime_origin_allowed(&request) {
        return Ok(axum::http::StatusCode::FORBIDDEN.into_response());
    }
    Ok(next.run(request).await)
}

#[cfg(feature = "stt")]
fn gateway_realtime_origin_allowed(request: &axum::extract::Request) -> bool {
    let mut values = request.headers().get_all(ORIGIN).iter();
    let first = values.next();
    if values.next().is_some() {
        return false;
    }
    let origin = match first {
        None => None,
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => return false,
        },
    };
    shared_loopback::gateway_loopback_origin_allowed(origin)
}

/// Header naming the caller for fair queue scheduling. Absent → `"default"`.
const CLIENT_HEADER: &str = "X-PromptForge-Client";

/// Resolves a request's model name against the live routing table.
///
/// A local model the boot load is spawning (one in [`LiveState::loading`])
/// earns [`GatewayError::ModelLoading`]: a 503 with `Retry-After`, because
/// the load will land it. A configured but not-yet-loaded model - one the
/// catalog names while the boot load is still downloading - earns a 503
/// naming the active queue command rather than a bare 404, so the caller
/// knows to retry once the command completes. With no command active the
/// miss is [`GatewayError::UnknownModel`], exactly as before the queue
/// existed.
async fn resolve_routed_model(
    state: &AppState,
    name: &str,
) -> Result<Arc<crate::routing::Model>, GatewayError> {
    let live = state.live.read().await;
    match live.routing.model(name) {
        Ok(model) => Ok(model),
        Err(unknown) => {
            if live.loading.contains(name) {
                return Err(GatewayError::ModelLoading(name.to_owned()));
            }
            let configured = live
                .config
                .models()
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
    caller: Caller,
    Json(request): Json<ChatRequest>,
) -> Result<Response, GatewayError> {
    check_auth(&state, &caller).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Chat)?;
    let client_id = crate::queue::ClientId::from_header(
        caller
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let permit = model.endpoint.queue.admit(client_id.as_str()).await?;
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
            let response = model
                .endpoint
                .upstream
                .send(buffered, &model.upstream_name)
                .await?;
            response
                .validate()
                .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
            let mut response = response;
            crate::dialect::apply_response(&mut response, &model.name);
            return Ok(relay_sse(crate::dialect::response_chunks(response), permit));
        }
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

    let relayed = futures_util::stream::unfold(
        (streamed.chunks, false, permit),
        |(mut chunks, failed, permit)| async move {
            if failed {
                return None;
            }
            let (line, failed) = match chunks.next().await? {
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
            };
            Some((
                Ok::<String, std::convert::Infallible>(line),
                (chunks, failed, permit),
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
    caller: Caller,
    Json(request): Json<EmbeddingRequest>,
) -> Result<Json<EmbeddingResponse>, GatewayError> {
    check_auth(&state, &caller).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Embedding)?;
    let client_id = crate::queue::ClientId::from_header(
        caller
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
    caller: Caller,
    Json(request): Json<RerankRequest>,
) -> Result<Json<RerankResponse>, GatewayError> {
    check_auth(&state, &caller).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Classifier)?;
    let client_id = crate::queue::ClientId::from_header(
        caller
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

/// The speech route to a backend: the same auth, routing, kind guard, and
/// dominion queue admission as chat, for `kind = "speech"` models.
///
/// Two deliberate departures from the other routes. First, auth runs before
/// body extraction: the handler takes the ungated [`Caller`] parts
/// extractor and a raw [`Request`], runs [`check_auth`], and only then
/// extracts `Json<SpeechRequest>` by hand, so an unauthorized caller never
/// makes the gateway parse a body. Second, the reply is a byte passthrough,
/// not a typed relay: audio frames are opaque bytes the gateway cannot
/// re-validate per chunk, so the upstream body is forwarded unread - the
/// one departure from the gateway's typed-relay norm, the same trade
/// [`relay_sse`] documents for its own design. Because the response is a
/// long-lived byte stream, this route must never sit under a
/// `CompressionLayer` or a whole-request `TimeoutLayer`: both buffer or
/// kill long-lived streams. The stream runs under the bounded background
/// relay [`relay_audio`] documents, so every early end - a tripped bound or
/// an upstream failure - fails the client's body read rather than
/// truncating it.
async fn audio_speech(
    State(state): State<AppState>,
    caller: Caller,
    request: Request,
) -> Result<Response, GatewayError> {
    check_auth(&state, &caller).await?;
    let Json(request) = Json::<SpeechRequest>::from_request(request, &state)
        .await
        .map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Speech)?;
    // A voice the model does not offer is a client error, so it is judged
    // before queue admission: a 400 never burns a queue slot.
    let voices = model.capabilities.voices();
    if !voices.is_empty() {
        let requested = match &request.voice {
            SpeechVoice::Name(name) => name.as_str(),
            SpeechVoice::Id { id } => id.as_str(),
        };
        if !voices.iter().any(|voice| voice == requested) {
            return Err(GatewayError::InvalidVoice {
                voice: requested.to_owned(),
                valid: voices.to_vec(),
            });
        }
    }
    let format = request.response_format;
    let stream_format = request.stream_format;
    let client_id = crate::queue::ClientId::from_header(
        caller
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let permit = model.endpoint.queue.admit(client_id.as_str()).await?;
    // A failure here is before the response starts, so it is consumed as a
    // normal JSON error, never a stream that dies mid-flight. The speech
    // path maps upstream 429/503 to its own envelope codes; every other
    // error keeps the shared protocol mapping.
    let streamed = model
        .endpoint
        .upstream
        .send_speech(request, &model.upstream_name)
        .await
        .map_err(|error| match error {
            ProtocolError::UpstreamStatus { status: 429, .. } => GatewayError::UpstreamRateLimited,
            ProtocolError::UpstreamStatus { status: 503, .. } => GatewayError::UpstreamUnavailable,
            other => GatewayError::Protocol(other),
        })?;
    Ok(relay_audio(streamed, format, stream_format, permit))
}

/// Total lifetime of one speech relay, headers to terminal end: a bound on
/// streams that would otherwise outlive every other budget by dripping.
#[cfg(not(any(test, feature = "test-fixtures")))]
const SPEECH_RELAY_TOTAL_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// Test-scaled so the relay boundary tests run in milliseconds.
#[cfg(any(test, feature = "test-fixtures"))]
const SPEECH_RELAY_TOTAL_LIFETIME: Duration = Duration::from_secs(2);

/// Ceiling on the response bytes one speech relay forwards.
#[cfg(not(any(test, feature = "test-fixtures")))]
const SPEECH_RELAY_BYTE_CEILING: u64 = 1 << 30;
/// Test-scaled so the relay boundary tests run in milliseconds.
#[cfg(any(test, feature = "test-fixtures"))]
const SPEECH_RELAY_BYTE_CEILING: u64 = 16 * 1024 * 1024;

/// Per-read idle budget on the opened upstream body: a silent upstream ends
/// the stream within this window. Time-to-headers is never governed here;
/// it is the upstream layer's first-response budget.
#[cfg(not(any(test, feature = "test-fixtures")))]
const SPEECH_RELAY_UPSTREAM_IDLE: Duration = Duration::from_secs(30);
/// Test-scaled so the relay boundary tests run in milliseconds.
#[cfg(any(test, feature = "test-fixtures"))]
const SPEECH_RELAY_UPSTREAM_IDLE: Duration = Duration::from_millis(200);

/// Budget for one blocked channel send: a downstream that stops reading
/// backpressures the bounded channel, and the relay ends the stream rather
/// than holding the permit forever.
#[cfg(not(any(test, feature = "test-fixtures")))]
const SPEECH_RELAY_DOWNSTREAM_BLOCKED: Duration = Duration::from_secs(60);
/// Test-scaled so the relay boundary tests run in milliseconds.
#[cfg(any(test, feature = "test-fixtures"))]
const SPEECH_RELAY_DOWNSTREAM_BLOCKED: Duration = Duration::from_millis(400);

/// Data chunks buffered between the relay task and the HTTP body. The
/// channel is built one slot larger: the extra slot is reserved up front
/// for the terminal error item, so delivering it never waits on the
/// downstream.
const SPEECH_RELAY_CHANNEL_CAPACITY: usize = 4;

/// Re-emit an upstream audio byte stream as the response body, holding the
/// dominion queue permit for the stream's lifetime.
///
/// The relay is untyped on purpose: audio frames are opaque bytes, so the
/// chunks pass through unread rather than being validated and re-serialized
/// the way [`relay_sse`] re-emits chat chunks. The response forwards the
/// upstream `Content-Type` when present and otherwise falls back to the
/// requested format's MIME mapping (or `text/event-stream` when the framing
/// selector is `sse`); `Content-Length` is never set, so hyper emits
/// `Transfer-Encoding: chunked`.
///
/// The forwarding runs in a spawned task that owns the upstream body and
/// the permit, feeding a small bounded channel the HTTP body consumes, so
/// the permit's lifetime never depends on downstream polling. Four named
/// bounds cap the stream: [`SPEECH_RELAY_TOTAL_LIFETIME`],
/// [`SPEECH_RELAY_BYTE_CEILING`], [`SPEECH_RELAY_UPSTREAM_IDLE`], and
/// [`SPEECH_RELAY_DOWNSTREAM_BLOCKED`].
///
/// No error envelope can follow 200 plus audio bytes, so every terminal
/// path - a bound tripped, an upstream body error, the downstream gone -
/// emits exactly one `Err` item into the channel, then drops the permit:
/// the client's body read fails rather than seeing a clean EOF, the same
/// fail-rather-than-truncate trade [`relay_sse`] makes with its mid-stream
/// error envelope. Over the wire a body-stream error aborts the response,
/// so the item's message is server-side diagnostics; the client observes a
/// failed read. Only a stream that ran to a clean upstream end inside every
/// bound ends the body without an error item.
fn relay_audio(
    streamed: crate::upstream::StreamedAudio,
    format: SpeechResponseFormat,
    stream_format: Option<SpeechStreamFormat>,
    permit: crate::queue::Permit,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel(SPEECH_RELAY_CHANNEL_CAPACITY + 1);
    tokio::spawn(relay_speech_stream(streamed.body, tx, permit));
    let relayed = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let mut response = Response::new(Body::from_stream(relayed));
    let content_type = if streamed.content_type.is_empty() {
        speech_fallback_mime(format, stream_format)
    } else {
        HeaderValue::from_str(&streamed.content_type)
            .unwrap_or_else(|_| speech_fallback_mime(format, stream_format))
    };
    response.headers_mut().insert(CONTENT_TYPE, content_type);
    response
}

/// The relay task behind [`relay_audio`]: reads the upstream body under the
/// idle and total-lifetime budgets, forwards each chunk under the
/// blocked-delivery budget, and on every terminal path emits exactly one
/// `Err` item through the reserved channel slot before returning, which
/// drops the upstream body and the permit together. A clean upstream end
/// is the one exit with no error item.
async fn relay_speech_stream(
    mut body: futures_util::stream::BoxStream<'static, Result<Bytes, ProtocolError>>,
    tx: tokio::sync::mpsc::Sender<Result<Bytes, ProtocolError>>,
    permit: crate::queue::Permit,
) {
    use futures_util::StreamExt as _;

    // Reserve the terminal slot before the first send competes for the
    // channel: the terminal error item is delivered even when every data
    // slot is full.
    let error_slot = tx
        .clone()
        .try_reserve_owned()
        .unwrap_or_else(|_| unreachable!("a fresh channel always has capacity"));
    let deadline = tokio::time::Instant::now() + SPEECH_RELAY_TOTAL_LIFETIME;
    let mut total_bytes: u64 = 0;
    let terminal: Option<ProtocolError> = 'relay: loop {
        let item = tokio::select! {
            item = tokio::time::timeout(SPEECH_RELAY_UPSTREAM_IDLE, body.next()) => item,
            () = tokio::time::sleep_until(deadline) => {
                break 'relay Some(relay_terminal("speech relay total lifetime exceeded"));
            }
            () = tx.closed() => {
                break 'relay Some(relay_terminal("speech relay downstream gone"));
            }
        };
        let chunk = match item {
            Ok(Some(Ok(chunk))) => chunk,
            // A clean upstream end inside every bound: the one exit with no
            // error item.
            Ok(None) => break 'relay None,
            // The upstream's own mid-stream failure is the terminal item.
            Ok(Some(Err(error))) => break 'relay Some(error),
            Err(_idle) => {
                break 'relay Some(relay_terminal("speech relay upstream idle"));
            }
        };
        total_bytes += u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        if total_bytes > SPEECH_RELAY_BYTE_CEILING {
            break 'relay Some(relay_terminal("speech relay byte ceiling exceeded"));
        }
        let delivered = tokio::select! {
            result = tokio::time::timeout(SPEECH_RELAY_DOWNSTREAM_BLOCKED, tx.send(Ok(chunk))) => {
                result
            }
            () = tokio::time::sleep_until(deadline) => {
                break 'relay Some(relay_terminal("speech relay total lifetime exceeded"));
            }
            () = tx.closed() => {
                break 'relay Some(relay_terminal("speech relay downstream gone"));
            }
        };
        match delivered {
            Ok(Ok(())) => {}
            // The downstream is gone mid-send or past the blocked budget.
            Ok(Err(_closed)) => {
                break 'relay Some(relay_terminal("speech relay downstream gone"));
            }
            Err(_elapsed) => {
                break 'relay Some(relay_terminal("speech relay downstream blocked"));
            }
        }
    };
    if let Some(error) = terminal {
        // The reserved slot makes this send immediate; when the downstream
        // is already gone the item is simply discarded.
        let _ = error_slot.send(Err(error));
    }
    // Explicit about the ownership the task exists for: the upstream body
    // and the dominion permit are released together on every exit, so no
    // path can end the stream while holding the slot.
    drop((body, permit));
}

/// The relay's terminal condition as a transport-classified protocol error:
/// the request may have reached the provider, and the message is
/// server-side diagnostics (a body-stream error aborts the response, so the
/// client observes a failed read, never this text).
fn relay_terminal(message: &'static str) -> ProtocolError {
    ProtocolError::transport(std::io::Error::other(message))
}

/// The `Content-Type` a speech response falls back to when the upstream
/// omits it or sends an invalid one: the framing selector first, so an SSE
/// stream is labeled `text/event-stream` and never an audio type, then the
/// requested format's MIME type (the OpenAI spellings).
fn speech_fallback_mime(
    format: SpeechResponseFormat,
    stream_format: Option<SpeechStreamFormat>,
) -> HeaderValue {
    if matches!(stream_format, Some(SpeechStreamFormat::Sse)) {
        return HeaderValue::from_static("text/event-stream");
    }
    HeaderValue::from_static(match format {
        SpeechResponseFormat::Mp3 => "audio/mpeg",
        SpeechResponseFormat::Opus => "audio/ogg",
        SpeechResponseFormat::Aac => "audio/aac",
        SpeechResponseFormat::Flac => "audio/flac",
        SpeechResponseFormat::Wav => "audio/wav",
        SpeechResponseFormat::Pcm => "audio/pcm",
    })
}

/// Bearer-authed union of the active profile's speech voices for host
/// bind: every `kind = "speech"` model's configured `voices`, deduplicated
/// and sorted, as id-first `{"id", "name"}` entries under
/// `{"voices": [...]}`.
///
/// OpenAI has no voice-list route; the OpenAI-compatible ecosystem
/// (Kokoro-FastAPI, vLLM-Omni, Fish Audio) converged on this one, and
/// clients such as Open WebUI read the `id` key, so the entry shape is a
/// compatibility surface pinned by the integration suite. `name` mirrors
/// `id`: the catalog configures voices as bare strings with no separate
/// display name.
async fn audio_voices(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let live = state.live.read().await;
    let voices = live
        .routing
        .models()
        .iter()
        .filter(|model| model.kind == ModelKind::Speech)
        .flat_map(|model| model.capabilities.voices().iter())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|voice| serde_json::json!({ "id": voice, "name": voice }))
        .collect::<Vec<_>>();
    drop(live);
    Ok(Json(serde_json::json!({ "voices": voices })))
}

/// Bearer-authed catalog of configured models for host bind.
async fn list_models(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<model_info::CatalogModelsResponse>, GatewayError> {
    check_auth(&state, &caller).await?;
    let live = state.live.read().await;
    let data = live
        .routing
        .models()
        .iter()
        .map(|model| {
            model_info::CatalogModelInfo::inference(ModelInfo {
                id: model.name.clone(),
                object: "model",
                kind: model.kind,
                description: model.description.clone(),
                context: model.context,
                thinking: model.thinking,
                capabilities: model.capabilities.clone(),
            })
        })
        .collect::<Vec<_>>();
    drop(live);
    #[cfg(feature = "stt")]
    let data = {
        let mut data = data;
        let speech_models = state.speech.models();
        data.extend(
            speech_models
                .iter()
                .map(model_info::CatalogModelInfo::speech),
        );
        data
    };
    Ok(Json(model_info::CatalogModelsResponse {
        object: "list",
        data,
    }))
}

/// The `POST /admin/switch-profile` body: a profile name, or `null` (or
/// absent) to select no profile.
#[derive(Debug, Deserialize)]
struct SwitchProfileRequest {
    name: Option<String>,
}

/// Lists profile names from the loaded global catalog.
async fn admin_list_profiles(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let live = state.live.read().await;
    let profiles: Vec<&str> = live
        .config
        .profiles()
        .iter()
        .map(gateway_config::ProfileConfig::name)
        .collect();
    Ok(Json(serde_json::json!({ "profiles": profiles })))
}

/// One capability endpoint's readout in the `GET /admin/status` response:
/// the route path, a display name, whether the live routing table serves
/// it, and whether a queue command is provisioning its models.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EndpointStatus {
    path: &'static str,
    name: &'static str,
    ready: bool,
    provisioning: bool,
}

/// Maps one capability endpoint's facts to its status entry. `ready` is
/// the routing table's live state; `provisioning` means the configuration
/// selects a model for the endpoint, none is loaded yet, and a queue
/// command is running. A configured endpoint with no command running reads
/// as simply not ready - the config UI's LED strip renders both
/// not-ready states gray and reserves amber for active provisioning.
fn endpoint_status(
    path: &'static str,
    name: &'static str,
    configured: bool,
    ready: bool,
    command_active: bool,
) -> EndpointStatus {
    EndpointStatus {
        path,
        name,
        ready,
        provisioning: configured && !ready && command_active,
    }
}

#[cfg(feature = "stt")]
fn with_speech_endpoint(
    mut endpoints: Vec<EndpointStatus>,
    speech: gateway_stt::SpeechStatus,
    command_active: bool,
) -> (Vec<EndpointStatus>, gateway_stt::SpeechStatus) {
    endpoints.push(endpoint_status(
        "/v1/audio/transcriptions",
        "Audio transcriptions",
        speech.configured(),
        speech.ready(),
        command_active,
    ));
    (endpoints, speech)
}

/// An `Instant` as Unix epoch seconds for the status wire shape. The
/// conversion goes through the elapsed duration, so a clock that jumped
/// backward clamps to now rather than underflowing.
fn instant_epoch_seconds(instant: std::time::Instant) -> u64 {
    let elapsed = instant.elapsed();
    std::time::SystemTime::now()
        .checked_sub(elapsed)
        .unwrap_or_else(std::time::SystemTime::now)
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Current profile name, loaded model names, the local models the boot
/// load is still spawning, process config generation, the profile's model
/// allowlist (its local and speech-to-text members), the declared VRAM
/// total, the command queue's active and pending commands, and one
/// readiness entry per capability endpoint the gateway can serve.
async fn admin_status(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let active = state.commands.active_command();
    let pending = state.commands.pending_commands();
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
    let (_, vram_gb) = live.model_status();
    let command_active = active.is_some();
    let configured = |kind: ModelKind| {
        live.config
            .models()
            .iter()
            .any(|model| model.kind() == kind)
            || live
                .config
                .catalog_local_models()
                .iter()
                .any(|model| model.kind() == kind)
    };
    let routed = |kind: ModelKind| live.routing.models().iter().any(|model| model.kind == kind);
    let endpoints = [
        ("/v1/chat/completions", "Chat completions", ModelKind::Chat),
        ("/v1/embeddings", "Embeddings", ModelKind::Embedding),
        ("/v1/rerank", "Rerank", ModelKind::Classifier),
        ("/v1/audio/speech", "Speech synthesis", ModelKind::Speech),
    ]
    .into_iter()
    .map(|(path, name, kind)| {
        endpoint_status(path, name, configured(kind), routed(kind), command_active)
    })
    .collect::<Vec<_>>();
    #[cfg(feature = "stt")]
    let (endpoints, speech) =
        with_speech_endpoint(endpoints, state.speech.status(), command_active);
    let response = serde_json::json!({
        "profile": live.profile_name,
        "models": models,
        "loading_models": live.loading.iter().collect::<Vec<_>>(),
        "config_generation": state.config_generation.as_ref(),
        "model_allowlist": live.model_allowlist,
        "local_children": local_children,
        "vram_gb": vram_gb,
        "queue": {
            "active": active.map(|status| serde_json::json!({
                "name": status.name,
                "fraction": status.progress,
                "started_at": instant_epoch_seconds(status.started_at),
            })),
            "pending": pending
                .iter()
                .map(|entry| serde_json::json!({
                    "name": entry.name,
                    "queued_at": instant_epoch_seconds(entry.queued_at),
                }))
                .collect::<Vec<_>>(),
        },
        "endpoints": endpoints
            .iter()
            .map(|endpoint| serde_json::json!({
                "path": endpoint.path,
                "name": endpoint.name,
                "ready": endpoint.ready,
                "provisioning": endpoint.provisioning,
            }))
            .collect::<Vec<_>>(),
    });
    #[cfg(feature = "stt")]
    let response = {
        let mut response = response;
        response["speech"] = serde_json::json!(system::SpeechSnapshot::from(speech));
        response
    };
    Ok(Json(response))
}

/// The `POST /admin/queue/cancel` route: bearer-authed, fires the active
/// command's cancellation token. The reply reports whether a command was
/// active to cancel; the command settles as cancelled at its next chunk
/// or phase boundary.
async fn admin_queue_cancel(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let cancelled = state.commands.cancel_active();
    Ok(Json(serde_json::json!({ "cancelled": cancelled })))
}

/// The `POST /admin/queue/cancel-pending` request body.
#[derive(Debug, Deserialize)]
struct CancelPendingRequest {
    index: usize,
}

/// The `POST /admin/queue/cancel-pending` route: bearer-authed, removes
/// the waiting command at `index`, settling its waiters as cancelled. The
/// reply reports whether an entry was removed.
async fn admin_queue_cancel_pending(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<CancelPendingRequest>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let cancelled = state.commands.cancel_pending(request.index);
    Ok(Json(serde_json::json!({ "cancelled": cancelled })))
}

/// The `GET /admin/config` route: bearer-authed, renders the running global
/// config plus its active profile in the pending admin shape.
async fn admin_config(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
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
/// [`ProgressEvent`], including one operation-level terminal event when a
/// tree detaches, with heartbeat comment lines every
/// [`PROGRESS_HEARTBEAT`] while the hub is idle. Intermediate events are
/// lossy - a lagging subscriber drops them - and terminal events are never
/// coalesced at the source. Client disconnect is Drop all the way down, as
/// with the switch stream: the response body owns the receiver. The one
/// server-side end is the process shutdown signal: an attached subscriber
/// (the config SPA, the workshop) would otherwise hold its connection open
/// through the graceful drain and pin the process.
async fn admin_progress(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Response, GatewayError> {
    check_auth(&state, &caller).await?;
    Ok(progress_sse_response(&state.hub, state.shutdown.clone()))
}

/// Builds the progress SSE response over `hub`: a snapshot of the live
/// operations first, then the broadcast stream, heartbeats in the gaps,
/// until `shutdown` fires.
fn progress_sse_response(hub: &ProgressHub, shutdown: shutdown::ShutdownSignal) -> Response {
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
            shutdown,
        ),
        |(mut pending, mut rx, mut heartbeat, shutdown)| async move {
            if let Some(line) = pending.pop_front() {
                return Some((
                    Ok::<_, std::convert::Infallible>(line),
                    (pending, rx, heartbeat, shutdown),
                ));
            }
            loop {
                tokio::select! {
                    () = shutdown.fired() => return None,
                    _ = heartbeat.tick() => {
                        return Some((Ok(": heartbeat\n\n".to_owned()), (pending, rx, heartbeat, shutdown)));
                    }
                    received = rx.recv() => match received {
                        Ok(event) => {
                            if let Some(line) = event_line(&event) {
                                return Some((Ok(line), (pending, rx, heartbeat, shutdown)));
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

/// Persists the profile selection and reports whether a restart is needed.
///
/// The local runtime is fixed for the process lifetime, so a switch never
/// starts or stops anything. A defined name is written to
/// `gateway.state.toml`; `null` deletes the file, the persisted form of
/// "no profile". The reply is `{"profile": Option<String>,
/// "restart_required": bool}`, where `restart_required` is whether the
/// selection differs from the running profile. The check against the live
/// catalog and the state write run under the apply lock, so
/// `GET /admin/config-pending` never reads a half-written selection.
///
/// A malformed name fails at the `parse-name` stage; an undefined one is
/// [`GatewayError::ProfileNotFound`] naming the defined profiles; a failed
/// state write is the config-write error. Every refusal changes nothing.
async fn admin_switch_profile(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<SwitchProfileRequest>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    let selected = request
        .name
        .as_deref()
        .map(ProfileName::parse)
        .transpose()
        .map_err(|e| GatewayError::switch_failed("parse-name", e))?;
    let _publication = state.apply.lock().await;
    let config_path = config_path(&state)?.to_path_buf();
    let restart_required = {
        let live = state.live.read().await;
        if let Some(name) = &selected {
            let defined: Vec<&str> = live
                .config
                .profiles()
                .iter()
                .map(gateway_config::ProfileConfig::name)
                .collect();
            if !defined.contains(&name.as_str()) {
                return Err(GatewayError::ProfileNotFound(undefined_profile_message(
                    name, &defined,
                )));
            }
        }
        selected.as_ref().map(ProfileName::as_str) != live.profile_name.as_deref()
    };
    let persisted = selected.clone();
    tokio::task::spawn_blocking(move || match &persisted {
        Some(name) => gateway_config::persist_profile_state(&config_path, name),
        None => gateway_config::clear_profile_state(&config_path),
    })
    .await
    .map_err(|join| GatewayError::ConfigWriteIo(Box::new(join)))?
    .map_err(config_write::config_write_error)?;
    Ok(Json(serde_json::json!({
        "profile": selected.as_ref().map(ProfileName::as_str),
        "restart_required": restart_required,
    })))
}

/// The `profile not found` detail for a switch to a name the live catalog
/// does not define: the name, then every defined profile so the operator
/// can pick one without a second request.
fn undefined_profile_message(name: &ProfileName, defined: &[&str]) -> String {
    if defined.is_empty() {
        format!("{name} (no profiles are defined)")
    } else {
        format!("{name} (defined profiles: {})", defined.join(", "))
    }
}

fn config_path(state: &AppState) -> Result<&std::path::Path, GatewayError> {
    state
        .config
        .as_ref()
        .map(|config| config.path.as_path())
        .ok_or(GatewayError::ConfigPathUnavailable)
}

/// Authenticates the caller by any one of three rules, in this order.
///
/// 1. A presented bearer token equals the live key.
/// 2. The `/auth` browser handoff's cookie verifies: the cookie is the
///    key's ambient form, accepted anywhere the bearer header is. Two
///    guards shape this path that the bearer path does not need: the
///    proof is recomputed from the process-lifetime salt and the live key
///    (the cookie never carries the key itself), and the request must
///    carry Fetch Metadata a cross-origin page cannot strip, since an
///    ambient credential would otherwise answer to any same-site loopback
///    page (ports are not part of a site).
/// 3. Loopback trust: `[server] trust_loopback` is on, the server recorded
///    a loopback peer for the connection, the request presents no
///    `Authorization` header at all, and its Fetch Metadata permits
///    ambient access ([`handoff::fetch_metadata_allows_ambient`]).
///
/// Two edges of rule 3 are deliberate. A presented-but-wrong bearer is
/// refused even on loopback: absence of credentials is what loopback
/// trusts, and a caller presenting wrong ones meant to authenticate - the
/// gateway-discovery-file liveness probe relies on that to detect a stale key.
/// And a request with no recorded peer address earns no trust: it needs
/// a credential, the same fail-closed posture as the loopback wall.
pub(crate) async fn check_auth(state: &AppState, caller: &Caller) -> Result<(), GatewayError> {
    let authorization = caller.get(AUTHORIZATION);
    let presented = authorization
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    let live = state.live.read().await;
    if secret_eq(presented.as_bytes(), live.key.expose().as_bytes()) {
        return Ok(());
    }
    if let Some(cookie) = handoff::presented_cookie_proof(caller)
        && handoff::fetch_metadata_allows_cookie(caller)
        && secret_eq(
            &cookie,
            &handoff::session_token(&state.handoff_salt, live.key.expose().as_bytes()),
        )
    {
        return Ok(());
    }
    if live.trust_loopback
        && authorization.is_none()
        && shared_loopback::is_loopback_peer(caller.peer())
        && handoff::fetch_metadata_allows_ambient(caller)
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
            "config-version = 0\n\
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
    async fn authenticated_multipart_rejection_uses_the_openai_error_envelope() {
        let response = build_router(state(), None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "not-multipart")
                    .body(Body::from("not multipart"))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
        assert_eq!(
            json,
            serde_json::json!({
                "error": {
                    "message": "malformed request: Invalid `boundary` for `multipart/form-data` request",
                    "type": "invalid_request_error",
                    "code": "malformed_request",
                }
            })
        );
    }

    #[tokio::test]
    async fn batch_validation_preserves_the_gateway_error_message_contract() {
        let body = "--empty\r\n\
                    Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\
                    Content-Type: audio/wav\r\n\r\n\
                    bytes\r\n\
                    --empty--\r\n";
        let response = build_router(state(), None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/transcriptions")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "multipart/form-data; boundary=empty")
                    .body(Body::from(body))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
        assert_eq!(
            json,
            serde_json::json!({
                "error": {
                    "message": "malformed request: missing multipart field model",
                    "type": "invalid_request_error",
                    "code": "malformed_request",
                }
            })
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
mod speech_auth_tests {
    //! The speech route's auth ordering through the real router. The route
    //! is unconditional, so these tests sit beside, not inside, the
    //! stt-gated `transcription_auth_tests` module.

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_config::Config;
    use tower::ServiceExt;

    use crate::build_router;
    use crate::test_support::app_state;

    fn state() -> crate::AppState {
        let config = Config::from_toml_str(
            "config-version = 0\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [workshop]\n",
        )
        .expect("config parses");
        app_state(config, None)
    }

    #[tokio::test]
    async fn speech_checks_bearer_auth_before_json_extraction() {
        let response = build_router(state(), None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/speech")
                    .header("content-type", "application/json")
                    .body(Body::from("{not json"))
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
    async fn authenticated_malformed_json_uses_the_openai_error_envelope() {
        let response = build_router(state(), None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/audio/speech")
                    .header("authorization", "Bearer test-token")
                    .header("content-type", "application/json")
                    .body(Body::from("{not json"))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
        assert_eq!(json["error"]["code"], "malformed_request");
        assert_eq!(json["error"]["type"], "invalid_request_error");
    }
}

#[cfg(test)]
mod tray_status_tests {
    use gateway_config::Config;

    use crate::test_support::app_state;

    /// Two remote models, one local model at 3.5 GB, one STT model at 1 GB,
    /// and a profile selecting both local entries.
    const CONFIG: &str = "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
         [[model]]\nname = \"alpha\"\ndescription = \"a\"\ncontext = 1024\nupstream = \"a\"\nendpoints = [\"fake\"]\n\
         [[model]]\nname = \"beta\"\ndescription = \"b\"\ncontext = 1024\nupstream = \"b\"\nendpoints = [\"fake\"]\n\
         [[local_model]]\nname = \"gamma\"\ndescription = \"g\"\nsource = \"/models/gamma.gguf\"\ncontext = 4096\nvram_gb = 3.5\n\
         [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n\
         [[profile]]\nname = \"work\"\nmodels = [\"gamma\", \"speech\"]\n";

    fn selected_config() -> Config {
        Config::from_toml_str(CONFIG)
            .expect("config parses")
            .select_profile(Some(
                &gateway_config::ProfileName::parse("work").expect("profile name"),
            ))
            .expect("the work profile selects")
    }

    /// With no local child running, only the boot STT selection counts.
    #[test]
    #[expect(clippy::float_cmp, reason = "1.0 is exact in binary floating point")]
    fn the_tray_status_counts_routed_models_and_the_boot_stt_selection() {
        let state = app_state(selected_config(), None);
        let (models, vram_gb) = state
            .tray_model_status()
            .expect("an uncontended state reads");
        assert_eq!(models, 2, "the harness routes the remote catalog");
        assert_eq!(
            vram_gb, 1.0,
            "no local child runs; the STT declaration counts"
        );
    }

    /// A running local child's declared VRAM counts, and keeps counting
    /// after an apply swaps the live config for one with no selection.
    #[cfg(feature = "test-fixtures")]
    #[tokio::test]
    #[expect(
        clippy::float_cmp,
        reason = "3.5 + 1.0 is exact in binary floating point"
    )]
    async fn the_status_sums_running_children_after_an_apply_swaps_the_config() {
        let state = app_state(selected_config(), None);
        let child = Config::from_toml_str(
            "config-version = 0\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [[endpoint]]\nid = \"local\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
             [[model]]\nname = \"gamma\"\ndescription = \"running child\"\ncontext = 4096\nupstream = \"gamma\"\nendpoints = [\"local\"]\n",
        )
        .expect("child config parses");
        let routing = crate::routing::Routing::from_config(&child).expect("child routes");
        state.live.write().await.local =
            crate::local::LocalRuntime::from_test_models(routing.models().to_vec());
        assert_eq!(state.live.read().await.model_status().1, 4.5);

        // What an apply publishes: the same document, parsed with no
        // profile selected, so `local_models()` and `stt_models()` are empty.
        let applied = Config::from_toml_str(CONFIG)
            .expect("config parses")
            .select_profile(None)
            .expect("no selection");
        assert!(applied.local_models().is_empty() && applied.stt_models().is_empty());
        state.live.write().await.config = std::sync::Arc::new(applied);

        let (models, vram_gb) = state.live.read().await.model_status();
        assert_eq!(models, 2);
        assert_eq!(
            vram_gb, 4.5,
            "the running child and the boot STT selection still count"
        );
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
            "config-version = 0\n\
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
                    Command::LoadProfile { name, token } => {
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

    /// Polls `condition` with a bounded wait.
    async fn wait_until(what: &str, condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !condition() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
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
            CancellationToken::new(),
        ));
        wait_until("the command to go active", || {
            state.commands.active_command().is_some()
        })
        .await;

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

    /// The resolver's miss ladder: a name in `loading` is `ModelLoading`,
    /// a name the catalog does not know stays `UnknownModel`.
    #[tokio::test]
    async fn a_routing_miss_on_a_loading_model_is_model_loading_not_not_found() {
        let state = state();
        state
            .live
            .write()
            .await
            .loading
            .insert("pending-model".to_owned());
        let loading = crate::resolve_routed_model(&state, "pending-model").await;
        assert!(
            matches!(&loading, Err(crate::error::GatewayError::ModelLoading(name)) if name == "pending-model"),
            "a loading model resolves to ModelLoading: {loading:?}"
        );
        let unknown = crate::resolve_routed_model(&state, "ghost").await;
        assert!(
            matches!(unknown, Err(crate::error::GatewayError::UnknownModel(_))),
            "a name outside loading keeps its 404: {unknown:?}"
        );
    }

    /// A profile over one remote model on `backend` and one local model
    /// whose source is a real file but whose `llama-server` is a plain text
    /// file, so the artifact step succeeds and the spawn fails per model.
    #[cfg(feature = "local")]
    fn local_profile_config(temp: &tempfile::TempDir, backend: &str) -> Config {
        let fake_server = temp.path().join("fake-llama-server");
        std::fs::write(&fake_server, b"not a server").expect("write fake server");
        let model_file = temp.path().join("local.gguf");
        std::fs::write(&model_file, b"not a gguf").expect("write model");
        let slash = |path: &std::path::Path| path.display().to_string().replace('\\', "/");
        Config::from_toml_str(&format!(
            "config-version = 0\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [local]\ncache_dir = '{}'\nllama_server_path = '{}'\n\
             [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\n\
             base_url = \"{backend}\"\napi_key = \"\"\n\
             [[model]]\nname = \"remote-model\"\ndescription = \"d\"\n\
             context = 8192\nupstream = \"backend-model\"\nendpoints = [\"e\"]\n\
             [[local_model]]\nname = \"local-model\"\ndescription = \"l\"\n\
             source = '{}'\ncontext = 4096\n\
             [[profile]]\nname = \"main\"\nmodels = [\"local-model\"]\n",
            slash(&temp.path().join("cache")),
            slash(&fake_server),
            slash(&model_file),
        ))
        .expect("config parses")
    }

    /// A fake OpenAI backend on an ephemeral loopback port answering every
    /// chat completion with a canned reply, so a routed request completes
    /// end to end.
    #[cfg(feature = "local")]
    async fn fake_chat_backend() -> std::net::SocketAddr {
        async fn completions(
            axum::Json(body): axum::Json<serde_json::Value>,
        ) -> axum::Json<serde_json::Value> {
            axum::Json(serde_json::json!({
                "id": "cmpl-test",
                "object": "chat.completion",
                "model": body["model"].as_str().unwrap_or(""),
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "pong" },
                    "finish_reason": "stop"
                }]
            }))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the backend listener binds");
        let addr = listener.local_addr().expect("the bound address");
        tokio::spawn(async move {
            let _ignored = axum::serve(
                listener,
                axum::Router::new().route("/chat/completions", axum::routing::post(completions)),
            )
            .await;
        });
        addr
    }

    #[cfg(feature = "local")]
    async fn status(state: AppState) -> serde_json::Value {
        let response = build_router(state, None)
            .oneshot(
                Request::builder()
                    .uri("/admin/status")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body reads"),
        )
        .expect("the status body is JSON")
    }

    /// The boot load over the runner's shell, parked at `phase` on the
    /// production worker: the state, the park, the worker, and the enqueued
    /// command's outcome.
    #[cfg(feature = "local")]
    async fn parked_boot_load(
        phase: crate::park::Phase,
    ) -> (
        AppState,
        Arc<crate::park::PhasePark>,
        tokio::task::JoinHandle<()>,
        tokio::sync::oneshot::Receiver<crate::commands::SharedOutcome>,
        tempfile::TempDir,
    ) {
        let backend = fake_chat_backend().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let mut state = crate::test_support::boot_state(local_profile_config(
            &temp,
            &format!("http://{backend}"),
        ));
        let park = Arc::new(crate::park::PhasePark::at(phase));
        state.park = Some(Arc::clone(&park));
        let worker = state
            .commands
            .spawn_worker(&state)
            .expect("the production worker spawns");
        let boot = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("main").expect("profile name"),
            CancellationToken::new(),
        ));
        tokio::time::timeout(Duration::from_secs(10), park.entered())
            .await
            .unwrap_or_else(|_| panic!("the boot load parks at {phase:?}"));
        (state, park, worker, boot.outcome, temp)
    }

    /// After the spawn failed: nothing is promised as loading, the local
    /// model is a plain 404, and the remote model keeps routing.
    #[cfg(feature = "local")]
    async fn assert_settled_after_failed_spawn(state: &AppState) {
        {
            let live = state.live.read().await;
            assert!(
                live.loading.is_empty(),
                "a failed spawn clears the loading set"
            );
            assert!(
                live.routing.model("remote-model").is_ok(),
                "the remote routing stays live after the failed spawn"
            );
            assert!(
                live.local.models().is_empty(),
                "no child is installed after the failed spawn"
            );
        }
        let response = chat(state.clone(), "local-model").await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "a model whose spawn failed is a 404, never a lingering 503"
        );
        assert_eq!(
            status(state.clone()).await["loading_models"],
            serde_json::json!([]),
            "the status lists nothing as loading after the spawn"
        );
        assert_eq!(
            chat(state.clone(), "remote-model").await.status(),
            StatusCode::OK,
            "the remote model keeps serving"
        );
    }

    /// The remote table the runner published serves end to end while the
    /// boot load downloads: nothing is promised as loading yet, so the
    /// local model answers 503 naming the boot command, and the status
    /// names the command as active. Released, the fake llama-server fails
    /// the spawn and the local model is a plain 404.
    #[cfg(feature = "local")]
    #[tokio::test]
    async fn a_remote_model_serves_while_the_boot_load_downloads() {
        let (state, park, worker, outcome, _temp) =
            parked_boot_load(crate::park::Phase::Download).await;

        assert_eq!(
            chat(state.clone(), "remote-model").await.status(),
            StatusCode::OK,
            "the remote model serves while the boot load downloads the local one"
        );
        assert!(
            state.live.read().await.loading.is_empty(),
            "nothing is promised as loading before the artifacts are staged"
        );
        let response = chat(state.clone(), "local-model").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
        assert_eq!(json["error"]["code"], "model_provisioning");
        assert_eq!(
            status(state.clone()).await["queue"]["active"]["name"],
            "load-profile: main",
            "the boot load is the active command"
        );

        park.release();
        let outcome = tokio::time::timeout(Duration::from_secs(30), outcome)
            .await
            .expect("the boot load settles")
            .expect("the worker settles the command");
        assert!(
            matches!(
                &*outcome,
                Err(crate::error::GatewayError::PartialStart { failed, .. })
                    if failed.iter().any(|entry| entry.starts_with("local-model"))
            ),
            "the fake llama-server cannot start the local model: {outcome:?}"
        );
        assert_settled_after_failed_spawn(&state).await;
        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }

    /// Once the artifacts are staged the local model is promised as
    /// loading: it answers 503 `model_loading` with `Retry-After`, the
    /// status lists it, the catalog lists only what routes, and the remote
    /// model serves throughout. Once the spawn fails the promise is
    /// withdrawn to a 404.
    #[cfg(feature = "local")]
    #[tokio::test]
    async fn a_loading_model_answers_503_until_its_spawn_settles() {
        let (state, park, worker, outcome, _temp) =
            parked_boot_load(crate::park::Phase::Spawn).await;

        assert_eq!(
            state.live.read().await.loading.iter().collect::<Vec<_>>(),
            ["local-model"],
            "the local model is promised as loading during the spawn"
        );
        let response = chat(state.clone(), "local-model").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("5"),
            "the 503 names the wait"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is JSON");
        assert_eq!(json["error"]["code"], "model_loading");
        let status = status(state.clone()).await;
        assert_eq!(status["loading_models"], serde_json::json!(["local-model"]));
        assert_eq!(status["models"], serde_json::json!(["remote-model"]));
        assert_eq!(
            chat(state.clone(), "remote-model").await.status(),
            StatusCode::OK
        );

        park.release();
        let outcome = tokio::time::timeout(Duration::from_secs(30), outcome)
            .await
            .expect("the boot load settles")
            .expect("the worker settles the command");
        assert!(
            matches!(
                &*outcome,
                Err(crate::error::GatewayError::PartialStart { .. })
            ),
            "the fake llama-server cannot start the local model: {outcome:?}"
        );
        assert_settled_after_failed_spawn(&state).await;
        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }

    /// A cancellation while the download is parked settles the boot load as
    /// cancelled and leaves the remote table serving: the local model is a
    /// 404, not a lingering 503.
    #[cfg(feature = "local")]
    #[tokio::test]
    async fn a_cancellation_during_the_download_keeps_the_remote_routing() {
        let (state, park, worker, outcome, _temp) =
            parked_boot_load(crate::park::Phase::Download).await;

        assert!(state.commands.cancel_active(), "the boot load is active");
        park.release();
        let outcome = tokio::time::timeout(Duration::from_secs(10), outcome)
            .await
            .expect("the cancelled command settles")
            .expect("the worker settles the command");
        assert!(
            matches!(
                &*outcome,
                Err(crate::error::GatewayError::CommandCancelled(_))
            ),
            "the download honors the cancellation: {outcome:?}"
        );
        assert_settled_after_failed_spawn(&state).await;
        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }
}

/// The boot `LoadProfile` command owns the process's one STT load: the
/// control plane serves complete responses while that load is parked,
/// speech becomes ready when it completes, and a later switch or Apply
/// persists new speech state without touching the running runtime.
#[cfg(all(test, feature = "stt"))]
mod boot_speech_tests {
    #![expect(
        clippy::expect_used,
        reason = "boot-path integration fixtures fail with the invariant named"
    )]

    use std::time::Duration;

    use gateway_config::{Config, ProfileName};
    use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    use tokio_tungstenite::tungstenite::http::HeaderValue;
    use tokio_util::sync::CancellationToken;

    use crate::commands::Command;
    use crate::test_support::{
        AdminPaths, GatedConstructionFactory, arm_boot_speech, boot_state_with_paths, serve_state,
    };

    const WAIT: Duration = Duration::from_secs(10);

    /// A catalog with one remote model per profile on the fake backend.
    fn catalog(backend: std::net::SocketAddr) -> String {
        format!(
            "config-version = 0\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [stt]\nwindow_seconds = 8\ninterval_ms = 250\nvocabulary = [\"alpha-words\"]\n\
             [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\nbase_url = \"http://{backend}\"\napi_key = \"\"\n\
             [[model]]\nname = \"alpha-model\"\ndescription = \"a\"\ncontext = 1024\nupstream = \"backend-model\"\nendpoints = [\"e\"]\n\
             [[model]]\nname = \"beta-model\"\ndescription = \"b\"\ncontext = 1024\nupstream = \"backend-model\"\nendpoints = [\"e\"]\n\
             [[profile]]\nname = \"alpha\"\nmodels = []\n\
             [[profile]]\nname = \"beta\"\nmodels = []\n"
        )
    }

    /// A fake OpenAI backend answering every chat completion with a canned
    /// reply, so a routed request completes end to end.
    async fn fake_chat_backend() -> std::net::SocketAddr {
        async fn completions(
            axum::Json(body): axum::Json<serde_json::Value>,
        ) -> axum::Json<serde_json::Value> {
            axum::Json(serde_json::json!({
                "id": "cmpl-test",
                "object": "chat.completion",
                "model": body["model"].as_str().unwrap_or(""),
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "pong" },
                    "finish_reason": "stop"
                }]
            }))
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the backend listener binds");
        let addr = listener.local_addr().expect("the bound address");
        tokio::spawn(async move {
            let _ignored = axum::serve(
                listener,
                axum::Router::new().route("/chat/completions", axum::routing::post(completions)),
            )
            .await;
        });
        addr
    }

    /// Writes the catalog and its state file, returning the fixture pieces.
    fn persisted_catalog(
        temp: &tempfile::TempDir,
        backend: std::net::SocketAddr,
        active: &str,
    ) -> (Config, AdminPaths) {
        let config_path = temp.path().join("gateway.toml");
        std::fs::write(&config_path, catalog(backend)).expect("write catalog");
        std::fs::write(
            gateway_config::profile_state_path(&config_path),
            format!("active_profile = \"{active}\"\n"),
        )
        .expect("write state");
        let config = Config::load(
            &config_path,
            &gateway_config::ProfileSelection::new(Some(active), None),
        )
        .expect("load the active profile");
        let paths = AdminPaths {
            fixture_dir: temp.path().to_path_buf(),
            active: active.to_owned(),
            config_path,
        };
        (config, paths)
    }

    /// Polls `condition` with a bounded wait.
    async fn wait_until(what: &str, condition: impl Fn() -> bool) {
        tokio::time::timeout(WAIT, async {
            while !condition() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    async fn get(addr: std::net::SocketAddr, path: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!("http://{addr}{path}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("GET sends")
    }

    async fn get_json(addr: std::net::SocketAddr, path: &str) -> serde_json::Value {
        let response = get(addr, path).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK, "GET {path}");
        response.json().await.expect("JSON body")
    }

    /// One batch transcription through the served router.
    async fn transcribe(addr: std::net::SocketAddr) -> reqwest::Response {
        const BOUNDARY: &str = "boot-speech-boundary";
        let wav = [
            b'R', b'I', b'F', b'F', 38, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
            16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0x00, 0x7d, 0, 0, 2, 0, 16, 0, b'd', b'a',
            b't', b'a', 2, 0, 0, 0, 0, 32,
        ];
        let mut body = format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n\
             scripted-interim\r\n\
             --{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"sample.wav\"\r\n\
             Content-Type: audio/wav\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&wav);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        reqwest::Client::new()
            .post(format!("http://{addr}/v1/audio/transcriptions"))
            .bearer_auth("test-token")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .body(body)
            .send()
            .await
            .expect("batch transcription sends")
    }

    /// The Realtime upgrade's status code: 101 once speech is ready with a
    /// final pass, 503 before. A successful upgrade is closed explicitly so
    /// the server-side session releases its admission before shutdown.
    async fn realtime_upgrade_status(addr: std::net::SocketAddr) -> u16 {
        let mut request = format!("ws://{addr}/v1/realtime?intent=transcription")
            .into_client_request()
            .expect("Realtime request builds");
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_static("Bearer test-token"),
        );
        match tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(request))
            .await
            .expect("the upgrade answers before the deadline")
        {
            Ok((mut socket, response)) => {
                let status = response.status().as_u16();
                socket.close(None).await.expect("the socket closes");
                status
            }
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                response.status().as_u16()
            }
            Err(other) => panic!("expected an upgrade or a refusal, got {other:?}"),
        }
    }

    fn speech_models(state: &crate::AppState) -> Vec<String> {
        state
            .speech
            .models()
            .iter()
            .map(|model| model.name().to_owned())
            .collect()
    }

    /// While the boot command is parked inside its STT load, every
    /// control-plane and ready non-STT route answers completely; releasing
    /// the load brings batch and Realtime to readiness.
    ///
    /// Multi-threaded: the closing `speech.shutdown()` blocks its calling
    /// thread while the served realtime session's cleanup runs on another.
    #[cfg(feature = "config-ui")]
    #[tokio::test(flavor = "multi_thread")]
    #[expect(
        clippy::too_many_lines,
        reason = "the single linear scenario probes every control-plane surface across the same parked-load boundary"
    )]
    async fn the_parked_boot_speech_load_never_blocks_the_control_plane() {
        let backend = fake_chat_backend().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let (config, paths) = persisted_catalog(&temp, backend, "alpha");
        let interim = ScriptedDecoder::new();
        interim.push_text("boot transcript");
        let gate = GatedConstructionFactory::new(
            ScriptedModelFactory::new(interim).with_final(ScriptedDecoder::new()),
        );
        let mut state = boot_state_with_paths(config, paths);
        arm_boot_speech(&mut state, gate.clone());
        let addr = serve_state(state.clone()).await;
        let worker = state.commands.spawn_worker(&state).expect("worker spawns");
        let boot = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("alpha").expect("profile name"),
            CancellationToken::new(),
        ));
        wait_until("the boot command to park inside the speech load", || {
            gate.entered()
        })
        .await;

        // The runner published the remote table before the boot load ran:
        // the remote model routes and answers end to end.
        let chat = reqwest::Client::new()
            .post(format!("http://{addr}/v1/chat/completions"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "alpha-model",
                "messages": [{"role": "user", "content": "ping"}]
            }))
            .send()
            .await
            .expect("chat sends");
        assert_eq!(chat.status(), reqwest::StatusCode::OK, "chat completes");
        let catalog = get_json(addr, "/v1/models").await;
        assert_eq!(
            catalog["data"]
                .as_array()
                .expect("catalog data")
                .iter()
                .map(|model| model["id"].as_str().expect("model id"))
                .collect::<Vec<_>>(),
            ["alpha-model", "beta-model"],
            "only the ready non-STT models are advertised"
        );

        let health = get(addr, "/health").await;
        assert_eq!(health.status(), reqwest::StatusCode::OK);
        let status = get_json(addr, "/admin/status").await;
        assert_eq!(status["profile"], "alpha");
        assert_eq!(
            status["speech"],
            serde_json::json!({"configured": false, "ready": false, "gpu": false}),
            "speech is not ready while its load is parked"
        );
        assert_eq!(
            status["queue"]["active"]["name"], "load-profile: alpha",
            "the queue status names the boot command doing the load"
        );

        let progress = get(addr, "/admin/progress").await;
        assert_eq!(progress.status(), reqwest::StatusCode::OK);
        assert_eq!(
            progress
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream"),
            "the progress stream serves while the load is parked"
        );

        let config_body = get_json(addr, "/admin/config").await;
        assert_eq!(config_body["active_profile"], "alpha");
        let profiles = get_json(addr, "/admin/profiles").await;
        assert_eq!(profiles["profiles"], serde_json::json!(["alpha", "beta"]));

        for (path, content_type) in [
            ("/config/", "text/html"),
            ("/config/app.js", "text/javascript"),
            ("/config/app.css", "text/css"),
            ("/config/icons/promptforge-icon.png", "image/png"),
        ] {
            let response = get(addr, path).await;
            assert_eq!(response.status(), reqwest::StatusCode::OK, "GET {path}");
            let served = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .expect("content type")
                .to_owned();
            assert!(
                served.starts_with(content_type),
                "GET {path} serves {content_type}, got {served}"
            );
            assert!(
                !response.bytes().await.expect("asset body").is_empty(),
                "GET {path} serves a complete body"
            );
        }

        // Speech routes keep their unavailable behavior while parked.
        let batch = transcribe(addr).await;
        assert_eq!(
            batch.status(),
            reqwest::StatusCode::NOT_FOUND,
            "batch transcription has no model before the load completes"
        );
        assert_eq!(realtime_upgrade_status(addr).await, 503);

        // Release the load: the boot command settles and speech is ready.
        gate.release();
        let outcome = tokio::time::timeout(WAIT, boot.outcome)
            .await
            .expect("the boot command settles")
            .expect("the worker settles the command");
        assert!(outcome.is_ok(), "the boot command succeeds: {outcome:?}");

        let status = get_json(addr, "/admin/status").await;
        assert_eq!(
            status["speech"],
            serde_json::json!({"configured": true, "ready": true, "gpu": false})
        );
        let catalog = get_json(addr, "/v1/models").await;
        assert_eq!(
            catalog["data"]
                .as_array()
                .expect("catalog data")
                .iter()
                .map(|model| model["id"].as_str().expect("model id"))
                .collect::<Vec<_>>(),
            [
                "alpha-model",
                "beta-model",
                "scripted-interim",
                "scripted-final",
                "realtime-transcribe"
            ]
        );
        let batch = transcribe(addr).await;
        assert_eq!(batch.status(), reqwest::StatusCode::OK, "batch is ready");
        let body: serde_json::Value = batch.json().await.expect("batch body");
        assert_eq!(body["text"], "boot transcript");
        assert_eq!(
            realtime_upgrade_status(addr).await,
            101,
            "Realtime is ready"
        );

        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
        state.speech.shutdown();
    }

    /// A failed boot STT load fails the boot command but never the gateway:
    /// the boot profile keeps serving, speech stays unavailable, and a
    /// later switch persists its selection without retrying the load or
    /// emitting a speech stage.
    #[tokio::test]
    async fn a_failed_boot_speech_load_leaves_the_gateway_serving_without_speech() {
        let backend = fake_chat_backend().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let (config, paths) = persisted_catalog(&temp, backend, "alpha");
        let mut state = boot_state_with_paths(config, paths);
        arm_boot_speech(
            &mut state,
            ScriptedModelFactory::new(ScriptedDecoder::new())
                .with_interim_failure("boot speech sentinel"),
        );
        let addr = serve_state(state.clone()).await;
        let worker = state.commands.spawn_worker(&state).expect("worker spawns");
        let boot = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("alpha").expect("profile name"),
            CancellationToken::new(),
        ));

        let outcome = tokio::time::timeout(WAIT, boot.outcome)
            .await
            .expect("the boot command settles")
            .expect("the worker settles the command");
        let chain = match &*outcome {
            Ok(profile) => panic!("the STT failure fails the boot command, got {profile}"),
            Err(error) => crate::config_write::error_chain(error),
        };
        assert!(chain.contains("load-speech"), "the stage is named: {chain}");
        assert!(
            chain.contains("boot speech sentinel"),
            "the cause is preserved: {chain}"
        );
        assert!(
            !state.shutdown.is_fired(),
            "an STT failure never requests gateway shutdown"
        );

        let health = get(addr, "/health").await;
        assert_eq!(health.status(), reqwest::StatusCode::OK);
        let status = get_json(addr, "/admin/status").await;
        assert_eq!(status["profile"], "alpha", "the boot profile serves");
        assert_eq!(
            status["speech"],
            serde_json::json!({"configured": false, "ready": false, "gpu": false})
        );
        let catalog = get_json(addr, "/v1/models").await;
        assert_eq!(
            catalog["data"]
                .as_array()
                .expect("catalog data")
                .iter()
                .map(|model| model["id"].as_str().expect("model id"))
                .collect::<Vec<_>>(),
            ["alpha-model", "beta-model"],
            "speech discovery stays empty until restart"
        );

        // A later switch persists its selection without a speech stage and
        // without retrying the spent initial load.
        let mut events = state.hub.subscribe();
        let switching = reqwest::Client::new()
            .post(format!("http://{addr}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": "beta" }))
            .send()
            .await
            .expect("the switch request sends");
        assert_eq!(switching.status(), reqwest::StatusCode::OK);
        assert!(
            !begun_stages_any(&mut events).contains(&"loading-speech".to_owned()),
            "a switch emits no speech stage"
        );
        assert!(!state.speech.status().ready());

        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }

    /// Cancelling the boot command while its STT load is parked settles the
    /// command as cancelled, spends the one attempt, and lets the worker
    /// exit on queue shutdown.
    #[tokio::test]
    async fn cancelling_the_parked_boot_speech_load_settles_cancelled() {
        let backend = fake_chat_backend().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let (config, paths) = persisted_catalog(&temp, backend, "alpha");
        let gate = GatedConstructionFactory::new(ScriptedModelFactory::new(ScriptedDecoder::new()));
        let mut state = boot_state_with_paths(config, paths);
        arm_boot_speech(&mut state, gate.clone());
        let addr = serve_state(state.clone()).await;
        let worker = state.commands.spawn_worker(&state).expect("worker spawns");
        let boot = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("alpha").expect("profile name"),
            CancellationToken::new(),
        ));
        wait_until("the boot command to park inside the speech load", || {
            gate.entered()
        })
        .await;

        assert!(state.commands.cancel_active(), "the boot command is active");
        gate.release();
        let outcome = tokio::time::timeout(WAIT, boot.outcome)
            .await
            .expect("the cancelled command settles")
            .expect("the worker settles the command");
        assert!(
            matches!(
                &*outcome,
                Err(crate::error::GatewayError::CommandCancelled(_))
            ),
            "the cancelled speech load settles the command as cancelled: {outcome:?}"
        );
        assert!(
            !state.speech.status().ready(),
            "the cancelled load published nothing"
        );
        let health = get(addr, "/health").await;
        assert_eq!(health.status(), reqwest::StatusCode::OK);

        state.commands.shutdown();
        tokio::time::timeout(WAIT, worker)
            .await
            .expect("the worker exits on shutdown")
            .expect("the worker task joins");
    }

    /// Boot speech A keeps serving after a later switch persists B and
    /// reports a restart; a fresh process state over the persisted
    /// selection loads B.
    #[tokio::test]
    async fn a_later_switch_persists_b_while_boot_speech_a_keeps_serving() {
        let backend = fake_chat_backend().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let (config, paths) = persisted_catalog(&temp, backend, "alpha");
        let config_path = paths.config_path.clone();
        let interim = ScriptedDecoder::new();
        interim.push_text("boot alpha transcript");
        let mut state = boot_state_with_paths(config, paths);
        arm_boot_speech(&mut state, ScriptedModelFactory::new(interim));
        let addr = serve_state(state.clone()).await;
        let worker = state.commands.spawn_worker(&state).expect("worker spawns");
        let boot = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("alpha").expect("profile name"),
            CancellationToken::new(),
        ));
        let outcome = tokio::time::timeout(WAIT, boot.outcome)
            .await
            .expect("the boot command settles")
            .expect("the worker settles the command");
        assert!(outcome.is_ok(), "boot A loads: {outcome:?}");
        assert_eq!(speech_models(&state), ["scripted-interim"]);

        // Select beta through the admin route: the selection persists and
        // the reply asks for a restart; nothing running changes.
        let switching = reqwest::Client::new()
            .post(format!("http://{addr}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": "beta" }))
            .send()
            .await
            .expect("the switch request sends");
        assert_eq!(switching.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = switching.json().await.expect("the reply is JSON");
        assert_eq!(
            body,
            serde_json::json!({ "profile": "beta", "restart_required": true }),
            "the switch to beta persists and reports a restart"
        );

        assert_eq!(
            std::fs::read_to_string(gateway_config::profile_state_path(&config_path))
                .expect("read state"),
            "active_profile = \"beta\"\n",
            "the switch persisted B"
        );
        assert_eq!(
            speech_models(&state),
            ["scripted-interim"],
            "the running process stays on boot speech A"
        );
        let batch = transcribe(addr).await;
        assert_eq!(batch.status(), reqwest::StatusCode::OK);
        let batch_body: serde_json::Value = batch.json().await.expect("batch body");
        assert_eq!(
            batch_body["text"], "boot alpha transcript",
            "boot speech A still serves"
        );

        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
        state.speech.shutdown();

        // The restart: a fresh process state over the persisted selection
        // loads B through its own boot command.
        let config = Config::load(&config_path, &gateway_config::ProfileSelection::default())
            .expect("the persisted selection loads");
        let paths = AdminPaths {
            fixture_dir: temp.path().to_path_buf(),
            active: "beta".to_owned(),
            config_path: config_path.clone(),
        };
        let mut restarted = boot_state_with_paths(config, paths);
        arm_boot_speech(
            &mut restarted,
            ScriptedModelFactory::new(ScriptedDecoder::new()).with_final(ScriptedDecoder::new()),
        );
        let worker = restarted
            .commands
            .spawn_worker(&restarted)
            .expect("worker spawns");
        let boot = restarted.commands.enqueue(Command::load_profile(
            ProfileName::parse("beta").expect("profile name"),
            CancellationToken::new(),
        ));
        let outcome = tokio::time::timeout(WAIT, boot.outcome)
            .await
            .expect("the restart boot command settles")
            .expect("the worker settles the command");
        assert!(outcome.is_ok(), "the restart boot loads: {outcome:?}");
        assert_eq!(
            speech_models(&restarted),
            ["scripted-interim", "scripted-final", "realtime-transcribe"],
            "the new process loads B"
        );
        restarted.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
        restarted.speech.shutdown();
    }

    /// An Apply that persists an STT settings change leaves the running
    /// boot runtime untouched and emits no speech stage.
    #[tokio::test]
    async fn an_apply_persisting_speech_changes_leaves_boot_speech_untouched() {
        let backend = fake_chat_backend().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let (config, paths) = persisted_catalog(&temp, backend, "alpha");
        let config_path = paths.config_path.clone();
        let interim = ScriptedDecoder::new();
        interim.push_text("boot alpha transcript");
        let mut state = boot_state_with_paths(config, paths);
        arm_boot_speech(&mut state, ScriptedModelFactory::new(interim));
        let addr = serve_state(state.clone()).await;
        let worker = state.commands.spawn_worker(&state).expect("worker spawns");
        let boot = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("alpha").expect("profile name"),
            CancellationToken::new(),
        ));
        let outcome = tokio::time::timeout(WAIT, boot.outcome)
            .await
            .expect("the boot command settles")
            .expect("the worker settles the command");
        assert!(outcome.is_ok(), "boot A loads: {outcome:?}");

        // Stage an STT vocabulary change through the real save route.
        let mut document = get_json(addr, "/admin/config").await;
        document
            .as_object_mut()
            .expect("the config is an object")
            .remove("active_profile");
        document["stt"]["vocabulary"] = serde_json::json!(["beta-words"]);
        let save = reqwest::Client::new()
            .put(format!("http://{addr}/admin/config"))
            .bearer_auth("test-token")
            .json(&document)
            .send()
            .await
            .expect("the save sends");
        assert_eq!(save.status(), reqwest::StatusCode::OK);

        let mut events = state.hub.subscribe();
        let apply = reqwest::Client::new()
            .post(format!("http://{addr}/admin/config-apply"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the apply sends");
        assert_eq!(apply.status(), reqwest::StatusCode::OK);
        let reply: serde_json::Value = apply.json().await.expect("apply body");
        assert_eq!(reply["reloaded"], true);
        assert_eq!(
            reply["restart_required"], true,
            "the speech pipeline is read once at boot"
        );
        assert_eq!(reply["applied"], serde_json::json!(["gateway.toml"]));

        assert!(
            std::fs::read_to_string(&config_path)
                .expect("read applied config")
                .contains("beta-words"),
            "the apply persisted the new speech setting"
        );
        assert_eq!(
            speech_models(&state),
            ["scripted-interim"],
            "the running process stays on boot speech A"
        );
        let batch = transcribe(addr).await;
        let batch_body: serde_json::Value = batch.json().await.expect("batch body");
        assert_eq!(batch_body["text"], "boot alpha transcript");
        let statuses = state.commands.active_command();
        assert!(statuses.is_none(), "the apply command settled");
        assert!(
            !begun_stages_any(&mut events).contains(&"loading-speech".to_owned()),
            "the apply emits no speech stage"
        );

        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
        state.speech.shutdown();
    }

    /// Labels of every begun progress event, across operations.
    fn begun_stages_any(
        events: &mut tokio::sync::broadcast::Receiver<shared_progress::ProgressEvent>,
    ) -> Vec<String> {
        let mut labels = Vec::new();
        while let Ok(event) = events.try_recv() {
            if matches!(event.state, shared_progress::EventState::Begun { .. }) {
                labels.push(event.label.clone());
            }
        }
        labels
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

    use super::shutdown::ShutdownSignal;
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
        let response = progress_sse_response(&hub, ShutdownSignal::default());
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
        let response = progress_sse_response(&hub, ShutdownSignal::default());
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
        let response = progress_sse_response(&hub, ShutdownSignal::default());
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
    async fn a_subscriber_sees_when_the_complete_operation_detaches() {
        let hub = Arc::new(ProgressHub::new());
        let response = progress_sse_response(&hub, ShutdownSignal::default());
        let mut frames = response.into_body().into_data_stream();
        let tree = hub.operation();
        let operation = tree.operation();
        let leaf = tree.register("loading-profile", 1.0);
        leaf.complete();
        drop(tree);

        let events = read_until(&mut frames, |event| {
            matches!(event.state, EventState::OperationFinished)
        })
        .await;
        assert_eq!(
            events.last().map(|event| event.operation),
            Some(operation),
            "the terminal lifecycle event names the detached operation"
        );
    }

    #[tokio::test]
    async fn a_lagged_subscriber_drops_the_overflow_and_carries_on() {
        let hub = Arc::new(ProgressHub::new());
        let response = progress_sse_response(&hub, ShutdownSignal::default());
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
        let response = progress_sse_response(&hub, ShutdownSignal::default());
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

    #[tokio::test(start_paused = true)]
    async fn a_tree_drop_reports_completion_then_the_stream_goes_quiet() {
        let hub = Arc::new(ProgressHub::new());
        let response = progress_sse_response(&hub, ShutdownSignal::default());
        let mut frames = response.into_body().into_data_stream();

        let tree = hub.operation();
        let operation = tree.operation();
        let _leaf = tree.register("download", 1.0);
        let events = read_events(&mut frames, 1).await;
        assert!(matches!(events[0].state, EventState::Begun { .. }));

        drop(tree);
        let events = read_events(&mut frames, 1).await;
        assert_eq!(events[0].operation, operation);
        assert!(matches!(events[0].state, EventState::OperationFinished));
        // The first heartbeat is 15 s out, so nothing may arrive inside this
        // window after completion: an idle hub is otherwise silent.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), frames.next())
                .await
                .is_err(),
            "a dropped tree must leave the stream quiet until the next heartbeat"
        );
    }

    /// The shutdown signal ends the open-ended stream, so an attached
    /// subscriber cannot hold its connection through the graceful drain.
    #[tokio::test]
    async fn the_stream_ends_when_the_shutdown_signal_fires() {
        let hub = Arc::new(ProgressHub::new());
        let shutdown = ShutdownSignal::default();
        let response = progress_sse_response(&hub, shutdown.clone());
        let mut frames = response.into_body().into_data_stream();

        let tree = hub.operation();
        let _leaf = tree.register("download", 1.0);
        let events = read_events(&mut frames, 1).await;
        assert!(matches!(events[0].state, EventState::Begun { .. }));

        shutdown.fire();
        let end = tokio::time::timeout(FRAME_TIMEOUT, frames.next())
            .await
            .expect("the stream reacts to the signal within the frame timeout");
        assert!(
            end.is_none(),
            "the stream ends on shutdown instead of waiting for the heartbeat: {end:?}"
        );
    }
}

#[cfg(test)]
mod switch_route_tests {
    //! `POST /admin/switch-profile` persists the selection and reports
    //! whether a restart is needed; it never touches the live state.

    use std::sync::Arc;

    use gateway_config::{Config, ProfileSelection, profile_state_path};

    use crate::AppState;
    use crate::test_support::{AdminPaths, app_state, serve_state};

    const TWO_PROFILES: &str = "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\n\
         base_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
         [[model]]\nname = \"m\"\ndescription = \"d\"\n\
         context = 8192\nupstream = \"u\"\nendpoints = [\"e\"]\n\
         [[profile]]\nname = \"alpha\"\nmodels = []\n\
         [[profile]]\nname = \"beta\"\nmodels = []\n";

    /// Writes the two-profile catalog into `temp` with no state file and
    /// builds the state `alpha` runs under, as a `--profile alpha` boot
    /// would leave it. Returns the state and the state file's path.
    fn running_alpha(temp: &tempfile::TempDir) -> (AppState, std::path::PathBuf) {
        let config_path = temp.path().join("gateway.toml");
        std::fs::write(&config_path, TWO_PROFILES).expect("write catalog");
        let config = Config::load(&config_path, &ProfileSelection::new(Some("alpha"), None))
            .expect("alpha loads");
        let state = app_state(
            config,
            Some(AdminPaths {
                fixture_dir: temp.path().to_path_buf(),
                active: "alpha".to_owned(),
                config_path: config_path.clone(),
            }),
        );
        (state, profile_state_path(&config_path))
    }

    /// What a switch must leave alone: the routing table, the running
    /// profile, and the local children.
    struct LiveSnapshot {
        routing: Arc<crate::routing::Routing>,
        profile_name: Option<String>,
        #[cfg(feature = "local")]
        local_models: Vec<String>,
    }

    async fn snapshot(state: &AppState) -> LiveSnapshot {
        let live = state.live.read().await;
        LiveSnapshot {
            routing: Arc::clone(&live.routing),
            profile_name: live.profile_name.clone(),
            #[cfg(feature = "local")]
            local_models: live
                .local
                .models()
                .iter()
                .map(|model| model.name.clone())
                .collect(),
        }
    }

    async fn assert_live_unchanged(state: &AppState, before: &LiveSnapshot) {
        let after = snapshot(state).await;
        assert!(
            Arc::ptr_eq(&before.routing, &after.routing),
            "a switch never swaps the routing table"
        );
        assert_eq!(
            before.profile_name, after.profile_name,
            "a switch never changes the running profile"
        );
        #[cfg(feature = "local")]
        assert_eq!(
            before.local_models, after.local_models,
            "a switch never starts or stops local children"
        );
    }

    async fn switch(addr: std::net::SocketAddr, body: serde_json::Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{addr}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&body)
            .send()
            .await
            .expect("the switch request sends")
    }

    async fn get_json(addr: std::net::SocketAddr, path: &str) -> serde_json::Value {
        let response = reqwest::Client::new()
            .get(format!("http://{addr}{path}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK, "{path}");
        response.json().await.expect("the body is JSON")
    }

    #[tokio::test]
    async fn switching_to_a_different_defined_profile_persists_it_and_requires_a_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (state, state_path) = running_alpha(&temp);
        let before = snapshot(&state).await;
        let addr = serve_state(state.clone()).await;

        let response = switch(addr, serde_json::json!({ "name": "beta" })).await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("the reply is JSON");
        assert_eq!(
            body,
            serde_json::json!({ "profile": "beta", "restart_required": true })
        );
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("the state file is written"),
            "active_profile = \"beta\"\n"
        );
        assert_live_unchanged(&state, &before).await;
        assert_eq!(
            get_json(addr, "/admin/config-pending").await["profile"]["active_profile"],
            "beta",
            "the pending view reports the persisted selection"
        );
        assert_eq!(
            get_json(addr, "/admin/status").await["profile"],
            "alpha",
            "the status readout reports the running profile"
        );
    }

    #[tokio::test]
    async fn switching_to_the_running_profile_persists_it_without_a_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (state, state_path) = running_alpha(&temp);
        let before = snapshot(&state).await;
        let addr = serve_state(state.clone()).await;

        let response = switch(addr, serde_json::json!({ "name": "alpha" })).await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("the reply is JSON");
        assert_eq!(
            body,
            serde_json::json!({ "profile": "alpha", "restart_required": false })
        );
        assert_eq!(
            std::fs::read_to_string(&state_path).expect("the state file is written"),
            "active_profile = \"alpha\"\n",
            "an ephemeral --profile boot becomes persisted by selecting it"
        );
        assert_live_unchanged(&state, &before).await;
    }

    #[tokio::test]
    async fn switching_to_an_undefined_profile_names_the_defined_ones_and_writes_nothing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (state, state_path) = running_alpha(&temp);
        let before = snapshot(&state).await;
        let addr = serve_state(state.clone()).await;

        let response = switch(addr, serde_json::json!({ "name": "ghost" })).await;

        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
        let body: serde_json::Value = response.json().await.expect("the error is JSON");
        assert_eq!(body["error"]["code"], "profile_not_found");
        let message = body["error"]["message"]
            .as_str()
            .expect("the message is a string");
        assert!(
            message.contains("ghost"),
            "names the refused profile: {message}"
        );
        assert!(
            message.contains("alpha, beta"),
            "names the defined profiles: {message}"
        );
        assert!(!state_path.exists(), "a refused switch writes no state");
        assert_live_unchanged(&state, &before).await;
        assert!(
            get_json(addr, "/admin/config-pending").await["profile"]["active_profile"].is_null(),
            "nothing is persisted"
        );
    }

    #[tokio::test]
    async fn a_malformed_name_is_refused_before_anything_is_written() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (state, state_path) = running_alpha(&temp);
        let addr = serve_state(state).await;

        let response = switch(addr, serde_json::json!({ "name": "../escape" })).await;

        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.expect("the error is JSON");
        assert_eq!(body["error"]["code"], "switch_failed");
        assert!(!state_path.exists(), "a malformed name writes no state");
    }

    #[tokio::test]
    async fn switching_to_null_deletes_the_state_file_and_requires_a_restart_when_a_profile_runs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (state, state_path) = running_alpha(&temp);
        std::fs::write(&state_path, "active_profile = \"alpha\"\n").expect("write state");
        let before = snapshot(&state).await;
        let addr = serve_state(state.clone()).await;

        let response = switch(addr, serde_json::json!({ "name": null })).await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("the reply is JSON");
        assert_eq!(
            body,
            serde_json::json!({ "profile": null, "restart_required": true })
        );
        assert!(
            !state_path.exists(),
            "an absent state file is the persisted form of no profile"
        );
        assert_live_unchanged(&state, &before).await;
        assert!(
            get_json(addr, "/admin/config-pending").await["profile"]["active_profile"].is_null(),
            "the pending view reports no persisted selection"
        );
        assert_eq!(
            get_json(addr, "/admin/status").await["profile"],
            "alpha",
            "the running profile is untouched"
        );
    }

    #[tokio::test]
    async fn switching_to_null_with_no_running_profile_needs_no_restart() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (state, state_path) = running_alpha(&temp);
        state.live.write().await.profile_name = None;
        let before = snapshot(&state).await;
        let addr = serve_state(state.clone()).await;

        let response = switch(addr, serde_json::json!({ "name": null })).await;

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("the reply is JSON");
        assert_eq!(
            body,
            serde_json::json!({ "profile": null, "restart_required": false })
        );
        assert!(!state_path.exists(), "no state file was ever written");
        assert_live_unchanged(&state, &before).await;
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
config-version = 0

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
        let requests = vec![
            (Method::GET, "/admin/config"),
            (Method::PUT, "/admin/config"),
            (Method::GET, "/admin/env"),
            (Method::PUT, "/admin/env"),
            (Method::GET, "/admin/config-pending"),
            (Method::GET, "/admin/config-dirty"),
            (Method::POST, "/admin/config-apply"),
            (Method::POST, "/admin/config-revert"),
            (Method::GET, "/admin/system"),
            (Method::GET, "/admin/cloud-models"),
            (Method::POST, "/admin/cloud-models/refresh"),
            (Method::GET, "/admin/hf/search?q=a&q=b"),
            (Method::GET, "/admin/hf/model/owner/na%20me"),
            (Method::POST, "/admin/reveal"),
        ];
        #[cfg(feature = "local")]
        let requests = {
            let mut requests = requests;
            requests.extend([
                (Method::GET, "/admin/chat-templates"),
                (Method::GET, "/admin/orphans"),
                (Method::GET, "/admin/model-info"),
            ]);
            requests
        };
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
            state.clone(),
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
        // The queue-cancel routes share the bearer-only, any-source
        // posture: cancelling a command mutates no configuration.
        for path in ["/admin/queue/cancel", "/admin/queue/cancel-pending"] {
            let status = send_with_peer(
                state.clone(),
                Method::POST,
                path,
                Some("198.51.100.7:44821"),
            )
            .await
            .status();
            assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "POST {path} keeps its bearer-only, any-source behavior"
            );
        }
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
        // `/health` is deliberately not exempt: the gateway-discovery-file probe
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

#[cfg(test)]
mod status_surface_tests {
    //! The status surfaces over the command queue: the `GET
    //! /admin/status` queue and endpoint readouts, and the queue-cancel
    //! routes firing the active command's token and dropping pending
    //! entries, exercised against a running fixture gateway with a
    //! parked command.

    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{Request, StatusCode};
    use futures_util::future::BoxFuture;
    use gateway_config::{Config, ProfileName};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use super::{EndpointStatus, endpoint_status};
    use crate::commands::{Command, Outcome};
    use crate::error::GatewayError;
    use crate::test_support::{app_state, serve_state};
    use crate::{AppState, build_router};

    /// A state whose catalog declares one local chat model the routing
    /// table never holds: `app_state` routes only the remote catalog, so
    /// `slow-model` stays configured-but-unloaded for the test's run.
    /// Strict bearer auth (`trust_loopback = false`): the cancel-route
    /// test pins that a missing key is refused from the loopback listener.
    fn state() -> AppState {
        let config = Config::from_toml_str(
            "config-version = 0\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             trust_loopback = false\n\
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
                    Command::LoadProfile { name, token } => {
                        token.cancelled().await;
                        Err(GatewayError::CommandCancelled(format!(
                            "load-profile: {name}"
                        )))
                    }
                    Command::ApplyConfig { token, .. } => {
                        token.cancelled().await;
                        Err(GatewayError::CommandCancelled(
                            crate::commands::APPLY_CONFIG_LABEL.to_owned(),
                        ))
                    }
                    Command::ProvisionModel { name, token, .. } => {
                        token.cancelled().await;
                        Err(GatewayError::CommandCancelled(format!(
                            "provision-model: {name}"
                        )))
                    }
                    Command::UnloadModel { name } => Ok(format!("unloaded {name}")),
                }
            }) as BoxFuture<'static, Outcome>
        })
    }

    /// Polls `condition` with a bounded wait, for observing the worker's
    /// externally visible state transitions.
    async fn wait_until(what: &str, condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !condition() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    async fn get_status(state: AppState) -> serde_json::Value {
        let response = build_router(state, None)
            .oneshot(
                Request::builder()
                    .uri("/admin/status")
                    .header(AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .expect("static request parts are valid"),
            )
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body reads"),
        )
        .expect("the status body is JSON")
    }

    #[test]
    fn the_endpoint_readiness_mapping() {
        let ready = endpoint_status("/v1/chat/completions", "Chat completions", true, true, true);
        assert_eq!(
            ready,
            EndpointStatus {
                path: "/v1/chat/completions",
                name: "Chat completions",
                ready: true,
                provisioning: false,
            },
            "a served endpoint is never provisioning, even mid-command"
        );
        assert!(
            endpoint_status("/v1/embeddings", "Embeddings", true, false, true).provisioning,
            "configured, unserved, and a command running: provisioning"
        );
        assert!(
            !endpoint_status("/v1/embeddings", "Embeddings", true, false, false).provisioning,
            "configured and unserved with an idle queue reads as not ready, not provisioning"
        );
        assert!(
            !endpoint_status("/v1/rerank", "Rerank", false, false, true).provisioning,
            "an unconfigured endpoint is never provisioning"
        );
    }

    #[tokio::test]
    async fn the_status_response_carries_the_queue_and_endpoint_shape() {
        let body = get_status(state()).await;
        assert_eq!(body["queue"]["active"], serde_json::Value::Null);
        assert_eq!(body["queue"]["pending"], serde_json::json!([]));
        assert_eq!(
            body["loading_models"],
            serde_json::json!([]),
            "with no switch running, nothing is loading: {body}"
        );
        assert!(
            body["vram_gb"].is_number(),
            "the declared VRAM total is always present: {body}"
        );
        let endpoints = body["endpoints"].as_array().expect("endpoints is an array");
        let chat = endpoints
            .iter()
            .find(|entry| entry["path"] == "/v1/chat/completions")
            .expect("the chat completions endpoint is listed");
        assert_eq!(chat["name"], "Chat completions");
        assert_eq!(
            chat["ready"], false,
            "the configured local model is not loaded"
        );
        assert_eq!(
            chat["provisioning"], false,
            "no command is running, so nothing provisions"
        );
        for path in ["/v1/embeddings", "/v1/rerank", "/v1/audio/speech"] {
            let entry = endpoints
                .iter()
                .find(|entry| entry["path"] == path)
                .unwrap_or_else(|| panic!("the {path} endpoint is listed"));
            assert_eq!(entry["ready"], false, "{path} has no configured model");
            assert_eq!(entry["provisioning"], false);
        }
        #[cfg(feature = "stt")]
        assert!(
            endpoints
                .iter()
                .any(|entry| entry["path"] == "/v1/audio/transcriptions"),
            "stt builds list the transcriptions endpoint"
        );
    }

    #[tokio::test]
    async fn the_status_response_reports_the_active_and_pending_commands() {
        let state = state();
        let queue = state.commands.clone();
        let worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");
        let active = queue.enqueue(Command::load_profile(
            ProfileName::parse("main").expect("profile name"),
            CancellationToken::new(),
        ));
        let pending = queue.enqueue(Command::ProvisionModel {
            name: "extra".to_owned(),
            source: "/models/extra.gguf".to_owned(),
            token: CancellationToken::new(),
        });
        wait_until("the boot command to go active", || {
            queue.active_command().is_some()
        })
        .await;

        let body = get_status(state.clone()).await;
        assert_eq!(
            body["queue"]["active"]["name"], "load-profile: main",
            "the active command is named: {body}"
        );
        assert!(
            body["queue"]["active"]["fraction"].is_number(),
            "the active command carries its progress fraction: {body}"
        );
        assert!(
            body["queue"]["active"]["started_at"].is_u64(),
            "the active command carries its start time as epoch seconds: {body}"
        );
        let pending_entries = body["queue"]["pending"]
            .as_array()
            .expect("pending is an array");
        assert_eq!(pending_entries.len(), 1);
        assert_eq!(pending_entries[0]["name"], "provision-model: extra");
        assert!(pending_entries[0]["queued_at"].is_u64());
        let chat = body["endpoints"]
            .as_array()
            .expect("endpoints is an array")
            .iter()
            .find(|entry| entry["path"] == "/v1/chat/completions")
            .expect("the chat completions endpoint is listed")
            .clone();
        assert_eq!(
            chat["provisioning"], true,
            "a configured, unloaded chat model under a running command is provisioning"
        );

        queue.cancel_active();
        drop((active, pending));
        queue.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }

    #[tokio::test]
    async fn the_cancel_routes_fire_the_token_and_drop_pending_entries() {
        let state = state();
        let queue = state.commands.clone();
        let worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");
        let addr = serve_state(state).await;
        let client = reqwest::Client::new();

        let active = queue.enqueue(Command::load_profile(
            ProfileName::parse("main").expect("profile name"),
            CancellationToken::new(),
        ));
        let pending = queue.enqueue(Command::ProvisionModel {
            name: "extra".to_owned(),
            source: "/models/extra.gguf".to_owned(),
            token: CancellationToken::new(),
        });
        wait_until("the boot command to go active", || {
            queue.active_command().is_some()
        })
        .await;

        // The routes refuse an unauthenticated caller before touching the
        // queue.
        let response = client
            .post(format!("http://{addr}/admin/queue/cancel"))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Cancelling a pending entry settles its waiter as cancelled and
        // leaves the active command running.
        let response = client
            .post(format!("http://{addr}/admin/queue/cancel-pending"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "index": 0 }))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("the reply is JSON");
        assert_eq!(body["cancelled"], true, "the pending entry was removed");
        let outcome = pending.outcome.await.expect("the pending waiter settles");
        assert!(
            matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
            "the cancelled pending command settles as cancelled: {outcome:?}"
        );
        assert!(
            queue.active_command().is_some(),
            "the active command still runs"
        );

        // Cancelling the active command fires its token; the parked body
        // observes it and settles as cancelled.
        let response = client
            .post(format!("http://{addr}/admin/queue/cancel"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("the reply is JSON");
        assert_eq!(body["cancelled"], true, "a command was active to cancel");
        let outcome = active.outcome.await.expect("the active waiter settles");
        assert!(
            matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
            "the parked command observed its token: {outcome:?}"
        );

        // With the queue idle, both routes report nothing to cancel.
        wait_until("the queue to go idle", || queue.active_command().is_none()).await;
        let response = client
            .post(format!("http://{addr}/admin/queue/cancel"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends");
        let body: serde_json::Value = response.json().await.expect("the reply is JSON");
        assert_eq!(body["cancelled"], false, "no active command to cancel");
        let response = client
            .post(format!("http://{addr}/admin/queue/cancel-pending"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "index": 0 }))
            .send()
            .await
            .expect("the request sends");
        let body: serde_json::Value = response.json().await.expect("the reply is JSON");
        assert_eq!(body["cancelled"], false, "no pending entry at the index");

        queue.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }
}
