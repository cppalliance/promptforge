//! PromptForge inference gateway.
//!
//! A small always-on service that accepts OpenAI-shaped chat completions, holds
//! the backend credential, resolves the request's model name to a configured
//! endpoint, forwards the request, and relays the reply. It is the only process
//! in the system with an edge to an LLM backend, so the executor above it never
//! holds a vendor key.
//!
//! What ships is an OpenAI-shaped inference surface and an admin surface
//! in two tiers, plus `GET /health`. The inference surface is
//! `POST /v1/chat/completions` (with a typed SSE relay for `stream:
//! true`), `POST /v1/embeddings`, `POST /v1/rerank`, `POST /v1/audio/speech`
//! and `GET /v1/audio/voices`, `GET /v1/models`, the Brave-backed
//! `POST /v1/tools/web_search` (`web-search` builds), the `/v1/cache` blob
//! cache (`local` builds), and, with the default-on `stt` feature, the
//! `POST /v1/audio/transcriptions` batch route and
//! `WS /v1/realtime?intent=transcription` served by the speech crate. All
//! of it is bearer-authed behind model routing and the shared dominion
//! queues (`[[dominion]]`); local generative inference runs as a managed
//! `llama-server` child (`[[local_model]]`).
//!
//! The admin surface's open tier - profiles and the profile switch, the
//! status readout, the progress stream, and queue cancellation - is
//! bearer-authed and reachable from any peer the listener admits. Its
//! walled tier - the config read and shadow-write routes, env, pending
//! state, apply and revert, host metrics, the Hugging Face proxy, the
//! cloud model sheet, reveal, `POST /shutdown`, and (in `config-ui`
//! builds) the embedded SPA at `/config/` with its `GET /auth?key=`
//! browser handoff, plus orphans, model-info, and chat-templates in
//! `local` builds - reads secrets in plaintext, writes files, or launches
//! processes, so it sits behind the shared loopback wall from
//! `shared-loopback` in every build. The one enumerable list of routes,
//! with the tier each belongs to, is the registry (`registry::all`); it
//! is logged at debug level when the router is built and swept by the
//! tests that prove each tier's wall.
//!
//! With `[server] trust_loopback` on (the default), a loopback peer
//! presenting no credential is admitted to every route unless its Fetch
//! Metadata marks a cross-origin page; `trust_loopback = false` requires
//! the bearer key from every caller. When the listener is bound to
//! loopback, every route additionally sits behind the shared
//! host-authority wall, which refuses requests whose `Host` is not the
//! bound socket (the DNS-rebinding defense). In-process llama.cpp FFI and
//! endpoint pinning are deferred.
//!
//! ## Where new route code goes
//!
//! A route area gets a module named after it (`relay`, `speech`,
//! `models`, `health`); a module earns a directory at three or more
//! files. Each area module owns its own mounts behind
//! `pub(crate) fn routes() -> Router<AppState>`, and `build_router` only
//! merges the areas and applies the walls, so a new endpoint never edits
//! this crate root: the handler and its `.route()` line go in the area's
//! module, and its tests in that module's kebab `-tests.rs` sibling,
//! wired with `#[path]`.
//!
//! The admin surface is split by tier, and the tier is the directory.
//! `admin/open/` holds the bearer-authed routes any admitted peer may
//! reach; `admin/walled/` holds the routes that read secrets in
//! plaintext, write files, or launch processes, which `build_router`
//! merges once behind the shared loopback wall. A handler under
//! `walled/` takes `LoopbackCaller` where an open handler takes
//! `AuthedCaller`, so its signature states the tier its path already
//! does. A new admin route picks its directory by that question and
//! nothing else.
//!
//! Handlers read live state through the `AppState` accessors
//! (`config()`, `routing()`, `profile_name()`), never by naming the
//! lock. A handler that needs several fields atomically takes one
//! scoped `state.live.read().await`; only commands and writers take the
//! write guard.

mod admin;
mod api_error;
mod auth;
mod boot;
mod boot_load;
#[cfg(feature = "local")]
mod cache;
mod commands;
mod diagnostics;
mod dialect;
mod error;
mod health;
mod models;
mod registry;
mod relaunch;
mod relay;
mod routing;
mod runner;
mod speech;
#[cfg(test)]
mod test_support;
mod tray;
#[cfg(feature = "web-search")]
mod web_search;

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

use axum::Router;
use tokio::sync::RwLock;

use crate::admin::AdminConfig;
use crate::admin::walled::{cloud_models, hf, reveal, shutdown, system};
#[cfg(feature = "local")]
use crate::local::LocalRuntime;
use crate::routing::Routing;
#[cfg(feature = "web-search")]
use gateway_config::WebSearchConfig;
use gateway_progress::ProgressHub;
#[cfg(feature = "stt")]
use gateway_stt::SpeechService;
#[cfg(feature = "web-search")]
use gateway_web_search::WebSearchState;

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
    /// The process-lifetime activity hub: commands and startup stages begin
    /// activities for their own lifetimes, `GET /admin/progress` streams its
    /// snapshots, and `GET /admin/status` and the tray read the current one.
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

    /// Builds full runtime state for `Gateway` and integration tests.
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

    /// The live routing table, shared by reference.
    pub(crate) async fn routing(&self) -> Arc<Routing> {
        Arc::clone(&self.live.read().await.routing)
    }

    /// The running configuration, shared by reference.
    pub(crate) async fn config(&self) -> Arc<Config> {
        Arc::clone(&self.live.read().await.config)
    }

    /// The running profile's name, when one is selected.
    pub(crate) async fn profile_name(&self) -> Option<String> {
        self.live.read().await.profile_name.clone()
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

    /// The hub's current activity text for the tray's status line: `Some`
    /// while any activity is live, `None` when the gateway is idle.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub(crate) fn tray_busy_text(&self) -> Option<String> {
        let progress = self.hub.current();
        progress.busy.then_some(progress.text)
    }
}

/// Builds the gateway's axum router.
///
/// `bound` is the socket the server actually bound. When it is loopback,
/// the whole surface is wrapped in the shared host-authority wall
/// ([`shared_loopback::require_loopback_host`]), the DNS-rebinding
/// defense; a non-loopback bind installs nothing, since a LAN server has
/// no loopback allowlist to enforce. The [`Gateway::router`] seam passes
/// `None` and carries no host wall: with no bound socket there is no
/// authority to allowlist.
pub(crate) fn build_router(state: AppState, bound: Option<std::net::SocketAddr>) -> Router {
    // The open tier: every area any admitted peer may reach, each mounted
    // by its own module. Feature-gated areas merge under the same gate
    // that compiles them, so a build with a feature off serves exactly
    // the space of a build with it on, minus that area.
    let router = Router::new()
        .merge(relay::routes())
        .merge(speech::routes())
        .merge(models::routes())
        .merge(health::routes())
        .merge(admin::open::routes());
    #[cfg(feature = "web-search")]
    let router = router.merge(web_search::routes());
    #[cfg(feature = "local")]
    let router = router.merge(cache::routes());
    // The walled tier reads secrets in plaintext, writes files, and
    // launches processes, so it sits behind the shared loopback wall in
    // every build: a non-loopback peer is refused with 403 before bearer
    // auth even runs. The wall is applied here, once, at the merge.
    let router = router.merge(
        admin::walled::routes()
            .route_layer(axum::middleware::from_fn(shared_loopback::require_loopback)),
    );
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
    // The registry is the enumerable form of the table just assembled;
    // logging it at debug level puts the mounted surface, tier by tier,
    // in the log of every build without anyone maintaining a list.
    for route in registry::all() {
        tracing::debug!(
            path = route.path,
            methods = ?route.methods,
            tier = ?route.tier,
            "route mounted"
        );
    }
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

#[cfg(test)]
#[path = "loopback-tests.rs"]
mod loopback_tests;
