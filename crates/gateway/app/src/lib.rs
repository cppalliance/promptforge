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
//!
//! ## Where new route code goes
//!
//! A route area gets a module named after it (`relay`, `speech`,
//! `models`, `admin`); a module earns a directory at three or more
//! files, the way `admin/` does. A new endpoint never edits this crate
//! root outside the route table in `build_router`: the handler goes in
//! its area's module, and its tests in that module's kebab `-tests.rs`
//! sibling, wired with `#[path]`.

mod admin;
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
mod models;
#[cfg(feature = "local")]
mod orphans;
mod relaunch;
mod relay;
mod render;
mod reveal;
mod routing;
mod runner;
mod shutdown;
mod speech;
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

use axum::Json;
#[cfg(feature = "web-search")]
use axum::extract::State;
#[cfg(feature = "local")]
use axum::routing::delete;
use axum::routing::{get, post};
use axum::{Router, response::IntoResponse};
use tokio::sync::RwLock;

use crate::admin::AdminConfig;
#[cfg(feature = "web-search")]
use crate::auth::AuthedCaller;
#[cfg(feature = "web-search")]
use crate::error::{GatewayError, WireJson};
#[cfg(feature = "local")]
use crate::local::LocalRuntime;
use crate::routing::Routing;
#[cfg(feature = "web-search")]
use gateway_config::WebSearchConfig;
#[cfg(feature = "stt")]
use gateway_stt::SpeechService;
#[cfg(feature = "web-search")]
use gateway_web_search::{WebSearchRequest, WebSearchResponse, WebSearchState};
use shared_progress::ProgressHub;

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
#[expect(
    clippy::too_many_lines,
    reason = "the route table is deliberately one screen: every mount, wall, and feature gate in a single scan"
)]
pub(crate) fn build_router(state: AppState, bound: Option<std::net::SocketAddr>) -> Router {
    let router = Router::new()
        .route("/v1/chat/completions", post(relay::chat_completions))
        .route("/v1/embeddings", post(relay::embeddings))
        .route("/v1/rerank", post(relay::rerank))
        .route("/v1/audio/speech", post(speech::audio_speech))
        .route("/v1/audio/voices", get(speech::audio_voices))
        .route("/v1/models", get(models::list_models))
        .route("/health", get(health))
        .route("/admin/profiles", get(admin::profiles::admin_list_profiles))
        .route("/admin/status", get(admin::status::admin_status))
        .route("/admin/progress", get(admin::progress::admin_progress))
        .route(
            "/admin/switch-profile",
            post(admin::profiles::admin_switch_profile),
        )
        .route(
            "/admin/queue/cancel",
            post(admin::queue::admin_queue_cancel),
        )
        .route(
            "/admin/queue/cancel-pending",
            post(admin::queue::admin_queue_cancel_pending),
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
            get(admin::config::admin_config).put(config_write::admin_put_config),
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
            auth::authorize_stt_route,
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
    _caller: AuthedCaller,
    WireJson(request): WireJson<WebSearchRequest>,
) -> Result<Json<WebSearchResponse>, GatewayError> {
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

#[cfg(test)]
#[path = "loopback-tests.rs"]
mod loopback_tests;
