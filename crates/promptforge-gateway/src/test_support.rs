//! Shared admin-route test harness: serves `build_router` over a state
//! assembled from one fixture profile.

use std::net::SocketAddr;
use std::sync::Arc;

use promptforge_gateway_config::Config;
use promptforge_progress::ProgressHub;

use crate::routing::Routing;
use crate::{AppState, BootOwned, ProfileSelection, build_router};

/// Serves `build_router` over a state assembled from `config` with no
/// running children: the retained config still carries everything the
/// admin routes read (the cache root, the `[[local_model]]` entries).
pub(crate) async fn serve(config: Config) -> SocketAddr {
    serve_with(config, None).await
}

/// Serves like [`serve`], but with the Hugging Face proxy replaced, so a
/// test can aim the `/admin/hf/*` routes at a local stub hub with an
/// explicit token instead of the process env.
pub(crate) async fn serve_with_hf(config: Config, hf: crate::hf::HfProxy) -> SocketAddr {
    serve_with(config, Some(hf)).await
}

/// The shared harness body behind [`serve`] and [`serve_with_hf`].
async fn serve_with(config: Config, hf: Option<crate::hf::HfProxy>) -> SocketAddr {
    let key = config.server_key();
    let server = config.server().clone();
    let routing = Routing::from_config(&config).expect("routing builds");
    let config = Arc::new(config);
    let mut state = AppState::from_parts(
        Arc::new(routing),
        key,
        Arc::clone(&config),
        #[cfg(feature = "local")]
        crate::local::LocalRuntime::empty(),
        #[cfg(feature = "web-search")]
        config.web_search_config(),
        None,
        ProfileSelection::default(),
        BootOwned {
            server,
            workshop: None,
        },
        Arc::new(ProgressHub::new()),
    );
    if let Some(hf) = hf {
        state.hf = Arc::new(hf);
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test listener binds");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(listener, build_router(state)).await;
    });
    addr
}
