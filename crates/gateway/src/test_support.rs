//! Shared admin-route test harness: serves `build_router` over a state
//! assembled from one fixture profile.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use gateway_config::Config;
use shared_progress::ProgressHub;

use crate::routing::Routing;
use crate::{AppState, ProfileSelection, build_router};

/// Filesystem context for shadow-write and env admin-route tests.
pub(crate) struct AdminPaths {
    /// Fixture directory used by filesystem-boundary tests.
    pub(crate) fixture_dir: PathBuf,
    /// The active profile name.
    pub(crate) active: String,
    /// The single config path.
    pub(crate) config_path: PathBuf,
}

/// Serves `build_router` over a state assembled from `config` with no
/// running children: the retained config still carries everything the
/// admin routes read (the cache root, the `[[local_model]]` entries).
pub(crate) async fn serve(config: Config) -> SocketAddr {
    serve_with(config, None, None).await
}

/// Serves like [`serve`], but with the Hugging Face proxy replaced, so a
/// test can aim the `/admin/hf/*` routes at a local stub hub with an
/// explicit token instead of the process env.
pub(crate) async fn serve_with_hf(config: Config, hf: crate::hf::HfProxy) -> SocketAddr {
    serve_with(config, Some(hf), None).await
}

/// Serves like [`serve`], but with active profile and config-file context.
pub(crate) async fn serve_with_paths(config: Config, paths: AdminPaths) -> SocketAddr {
    serve_with(config, None, Some(paths)).await
}

/// The shared harness body behind [`serve`], [`serve_with_hf`], and
/// [`serve_with_paths`].
async fn serve_with(
    config: Config,
    hf: Option<crate::hf::HfProxy>,
    paths: Option<AdminPaths>,
) -> SocketAddr {
    let mut state = app_state(config, paths);
    if let Some(hf) = hf {
        state.hf = Arc::new(hf);
    }
    serve_state(state).await
}

/// Builds the state the harness serves, so a test can override an
/// injected collaborator (the HF proxy, the reveal launcher) or drive a
/// handler directly.
pub(crate) fn app_state(config: Config, paths: Option<AdminPaths>) -> AppState {
    let key = config.server_key();
    let routing = Routing::from_config(&config).expect("routing builds");
    let config = Arc::new(config);
    let (config_path, selection) = match paths {
        Some(paths) => (
            Some(paths.config_path),
            ProfileSelection {
                name: Some(paths.active),
                model_allowlist: None,
            },
        ),
        None => (None, ProfileSelection::default()),
    };
    AppState::from_parts(
        Arc::new(routing),
        key,
        Arc::clone(&config),
        #[cfg(feature = "local")]
        crate::local::LocalRuntime::empty(),
        #[cfg(feature = "stt")]
        gateway_stt::SttRuntime::empty(gateway_stt::SttState::default()),
        #[cfg(feature = "web-search")]
        config.web_search_config(),
        config_path,
        selection,
        Arc::new(ProgressHub::new()),
    )
}

/// Binds an ephemeral loopback listener and serves `state` on it,
/// returning the bound address. Connect info and the host-authority wall
/// are wired exactly as in the production serve path, so loopback-only
/// routes see a peer address and the wall sees the bound socket.
pub(crate) async fn serve_state(state: AppState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test listener binds");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(
            listener,
            build_router(state, Some(addr)).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    addr
}
