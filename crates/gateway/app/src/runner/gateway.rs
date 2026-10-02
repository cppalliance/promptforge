//! The [`Gateway`] assembly paths, its router, and its serve loop.

use std::future::Future;
use std::sync::Arc;

use tokio::net::TcpListener;

use gateway_config::{Config, ProfileName};

use super::{GRACEFUL_DRAIN_TIMEOUT, Gateway, ProfilesContext, WORKER_JOIN_TIMEOUT};
use crate::api_error::{ServeError, StartupError};
#[cfg(feature = "local")]
use crate::local::LocalRuntime;
use crate::routing::Routing;
use crate::{AppState, build_router};

impl Gateway {
    /// Assembles the serving shell instantly: the routing table over every
    /// `[[model]]`, no local runtime, no provisioning. The selected
    /// profile's local models arrive when the command queue's boot
    /// `LoadProfile` merges them into the live table; until then an
    /// unloaded but configured local model receives a 503 naming the
    /// active command.
    ///
    /// [`from_config`](Self::from_config) is the eager alternative for tests
    /// and embedders: it provisions before returning.
    ///
    /// # Errors
    /// Returns [`StartupError`] when routing construction fails.
    pub fn new(config: &Config, profiles: ProfilesContext) -> Result<Gateway, StartupError> {
        Self::new_with_hub(
            config,
            profiles,
            Arc::new(gateway_progress::ProgressHub::new()),
        )
    }

    /// [`new`](Self::new) over a caller-provided progress hub, so the
    /// serving lifecycle's status consumers see the boot command's progress.
    pub(super) fn new_with_hub(
        config: &Config,
        profiles: ProfilesContext,
        hub: Arc<gateway_progress::ProgressHub>,
    ) -> Result<Gateway, StartupError> {
        let routing = Routing::from_config(config).map_err(StartupError::config)?;
        let active = config
            .active_profile()
            .map(|profile| profile.name().to_owned())
            .or_else(|| profiles.active.map(|name| name.to_string()));
        let model_allowlist = config
            .active_profile()
            .map(|profile| profile.models().to_vec());
        let state = AppState::from_parts(
            Arc::new(routing),
            config.server_key(),
            Arc::new(config.clone()),
            #[cfg(feature = "local")]
            LocalRuntime::empty(),
            #[cfg(feature = "stt")]
            gateway_stt::SpeechService::new(),
            #[cfg(feature = "web-search")]
            config.web_search_config(),
            profiles.config_path,
            crate::ProfileSelection {
                name: active,
                model_allowlist,
            },
            hub,
        );
        Ok(Gateway { state })
    }

    /// Enqueues the boot load for `profile`, or nothing when no profile is
    /// selected: the local runtime then stays empty for the process
    /// lifetime and the remote table published at assembly is the whole
    /// catalog. Returns whether a command was enqueued.
    pub(super) fn enqueue_boot_load(&self, profile: Option<ProfileName>) -> bool {
        let Some(name) = profile else {
            return false;
        };
        let _boot = self
            .state
            .commands
            .enqueue(crate::commands::Command::load_profile(
                name,
                tokio_util::sync::CancellationToken::new(),
            ));
        true
    }

    /// Assembles from a validated config. Provisions and starts local models.
    ///
    /// The boot selection is fixed for the process lifetime; a later switch
    /// persists a new selection and reports that a restart is needed.
    ///
    /// # Errors
    /// Returns [`StartupError`] when local provisioning or routing construction
    /// fails.
    pub fn from_config(
        config: &Config,
        profiles: ProfilesContext,
    ) -> Result<Gateway, StartupError> {
        Self::from_config_with_hub(
            config,
            profiles,
            Arc::new(gateway_progress::ProgressHub::new()),
        )
    }

    /// [`from_config`](Self::from_config) over a caller-provided progress
    /// hub, so the serving lifecycle's status consumers can watch startup
    /// provisioning.
    fn from_config_with_hub(
        config: &Config,
        profiles: ProfilesContext,
        hub: Arc<gateway_progress::ProgressHub>,
    ) -> Result<Gateway, StartupError> {
        // Startup provisioning is the hub's first activity: it lives for the
        // provisioning call and ends when the guard drops.
        #[cfg(feature = "local")]
        let local = {
            let activity = hub.begin("Starting local models");
            let started =
                LocalRuntime::start(config, Some(&activity)).map_err(StartupError::provisioning);
            drop(activity);
            started?
        };
        // A headless build cannot honor a config declaring local models;
        // refuse at assembly rather than silently dropping them.
        #[cfg(not(feature = "local"))]
        if !config.local_models().is_empty() {
            return Err(StartupError::provisioning(std::io::Error::other(
                crate::LOCAL_MODELS_UNSUPPORTED,
            )));
        }
        #[cfg(feature = "stt")]
        let speech = {
            let activity = Arc::new(hub.begin("Loading speech"));
            let service = gateway_stt::SpeechService::new();
            let started = service
                .load_initial(
                    config,
                    Some(&activity),
                    &tokio_util::sync::CancellationToken::new(),
                )
                .map(|()| service)
                .map_err(StartupError::provisioning);
            drop(activity);
            started?
        };
        #[cfg(not(feature = "stt"))]
        if !config.stt_models().is_empty() {
            return Err(StartupError::provisioning(std::io::Error::other(
                crate::STT_RUNTIME_UNAVAILABLE,
            )));
        }
        let routing = Routing::from_config(config).map_err(StartupError::config)?;
        #[cfg(feature = "local")]
        let routing = routing
            .merge(local.models().iter().cloned())
            .map_err(StartupError::config)?;
        let active = config
            .active_profile()
            .map(|profile| profile.name().to_owned())
            .or_else(|| profiles.active.map(|name| name.to_string()));
        let model_allowlist = config
            .active_profile()
            .map(|profile| profile.models().to_vec());
        let state = AppState::from_parts(
            Arc::new(routing),
            config.server_key(),
            Arc::new(config.clone()),
            #[cfg(feature = "local")]
            local,
            #[cfg(feature = "stt")]
            speech,
            #[cfg(feature = "web-search")]
            config.web_search_config(),
            profiles.config_path,
            crate::ProfileSelection {
                name: active,
                model_allowlist,
            },
            hub,
        );
        Ok(Gateway { state })
    }

    /// The Axum router for this gateway.
    ///
    /// This is the crate's one deliberate, documented Axum integration point;
    /// the crate is an application, not a general library, so exposing an
    /// [`axum::Router`] here is intentional.
    ///
    /// The router has no bound socket, so the host-authority wall is
    /// not installed; it exists on the [`serve`](Self::serve) path, where
    /// the bound address is known. Likewise `POST /shutdown` answers 202
    /// here without stopping anything: only `serve` selects on the
    /// route's signal.
    pub fn router(&self) -> axum::Router {
        build_router(self.state.clone(), None)
    }

    /// Replaces the speech facade used by routes and the boot command's one
    /// initial load.
    ///
    /// This composition seam lets embedders provide an already prepared
    /// speech generation while preserving the Gateway's authentication,
    /// host-authority, and route-layer policies.
    ///
    /// A graceful stop of [`serve`](Self::serve) retires the service it was
    /// given, clones included, so one service serves one `serve` call.
    #[cfg(feature = "stt")]
    #[must_use]
    pub fn with_speech_service(mut self, service: gateway_stt::SpeechService) -> Self {
        self.state.speech = service;
        self
    }

    /// Bounded stdout/stderr tails captured from each running local
    /// `llama-server` child, keyed by configured model name.
    ///
    /// The per-attempt loopback credential is redacted from the captures.
    /// Embedders use this to verify what a child actually reported -
    /// that a CUDA build staged its embedded bundle, that the child saw a
    /// CUDA device, that model layers offloaded to the GPU - without
    /// reaching the child's private loopback port. Empty when the config
    /// declares no `[[local_model]]`.
    ///
    /// Available only in builds with the `local` feature.
    #[cfg(feature = "local")]
    pub async fn local_diagnostics(&self) -> Vec<(String, String)> {
        self.state.live.read().await.local.diagnostics()
    }

    /// Serves on a caller-owned listener until `shutdown` completes or
    /// `POST /shutdown` fires the route's own signal, whichever comes
    /// first; both drive the same graceful drain.
    ///
    /// Tests pass an ephemeral [`TcpListener`] they bound themselves (no port
    /// race), read back `local_addr`, and drive a rendezvous instead of
    /// sleeping.
    ///
    /// The command queue's worker task runs for the life of this call: boot
    /// provisioning, profile switches, and unloads all drain through it. A
    /// `Gateway` whose router is taken without serving
    /// ([`router`](Self::router)) has no worker, so its queue accepts but
    /// never runs commands.
    ///
    /// Shutdown fires the route signal (so every open-ended stream ends),
    /// closes the queue (so the active command cancels and nothing pending
    /// starts), then drains in-flight requests for at most
    /// `GRACEFUL_DRAIN_TIMEOUT`; a connection that outlives the drain is
    /// dropped with the runtime rather than pinning the exit. The command
    /// worker is then joined for at most `WORKER_JOIN_TIMEOUT`: a command
    /// body that ignored its cancellation token is abandoned to the runtime
    /// teardown instead of pinning the exit. Last, within the same
    /// deadline, the speech service retires: admission closes, which ends
    /// Realtime sessions and aborts running decodes after their current
    /// encoder pass or decoder step, admitted speech work drains, and the
    /// engine's workers are joined, so a native decoder frees its context
    /// before the process exits. A retirement still draining at the
    /// deadline is abandoned the same way.
    ///
    /// # Errors
    /// Returns [`ServeError`] when the bound address cannot be read or the
    /// HTTP server fails.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServeError> {
        // The configured bind may specify port 0; the bound address is what
        // the host-authority wall allowlists.
        let bound = listener.local_addr().map_err(ServeError::io)?;
        let state = self.state;
        let route_shutdown = state.shutdown.clone();
        let drain_shutdown = state.shutdown.clone();
        let commands = state.commands.clone();
        let commands_after = state.commands.clone();
        #[cfg(feature = "stt")]
        let speech = state.speech.clone();
        let worker = state.commands.spawn_worker(&state);
        // Connect info exposes each request's peer address, so
        // loopback-only routes (`POST /admin/reveal`) can tell loopback
        // callers from LAN callers.
        let server = axum::serve(
            listener,
            build_router(state, Some(bound))
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            tokio::select! {
                () = shutdown => {}
                () = route_shutdown.fired() => {}
            }
            // The caller-owned future may have fired without the route
            // signal; firing it here ends every stream that selects on it,
            // so the drain below is not held open by a progress subscriber.
            route_shutdown.fire();
            // Close the queue before the drain: the active command's token
            // fires so a quit during provisioning stops the download, and no
            // pending command starts under a server that is going away.
            commands.shutdown();
        });
        let server = std::future::IntoFuture::into_future(server);
        let result = tokio::select! {
            result = server => result.map_err(ServeError::io),
            () = async {
                drain_shutdown.fired().await;
                tokio::time::sleep(GRACEFUL_DRAIN_TIMEOUT).await;
            } => {
                tracing::warn!(
                    "graceful drain exceeded {GRACEFUL_DRAIN_TIMEOUT:?}; abandoning the remaining connections"
                );
                Ok(())
            }
        };
        // Close the queue and reap the worker, so a gateway served on a
        // caller-owned runtime (tests, embedders) leaves no task behind.
        // The join is bounded: a command body that ignored its token is
        // abandoned here and left to the runtime teardown bound. One
        // deadline bounds the join and the speech retirement after it.
        commands_after.shutdown();
        let deadline = tokio::time::Instant::now() + WORKER_JOIN_TIMEOUT;
        if let Some(worker) = worker
            && tokio::time::timeout_at(deadline, worker).await.is_err()
        {
            let command = commands_after
                .active_command()
                .map_or_else(|| "unknown".to_owned(), |status| status.name);
            tracing::warn!(
                command = %command,
                "the command worker did not stop within {WORKER_JOIN_TIMEOUT:?}; abandoning it"
            );
        }
        // Speech retires before `serve` returns, so the engine's workers
        // free their native contexts while the process is still whole:
        // CUDA memory freed during process exit fails with "driver
        // shutting down". The retirement blocks until admitted speech work
        // drains, so it runs on the blocking pool, and a drain still
        // waiting at the deadline is abandoned to the runtime teardown
        // bound like a stuck command.
        #[cfg(feature = "stt")]
        if tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || speech.shutdown()),
        )
        .await
        .is_err()
        {
            tracing::warn!("speech did not retire within {WORKER_JOIN_TIMEOUT:?}; abandoning it");
        }
        result
    }
}
