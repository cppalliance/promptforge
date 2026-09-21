//! The walled admin tier: routes that read secrets in plaintext, write
//! files, or launch processes. `build_router` mounts [`routes`] behind
//! the shared loopback wall from `shared-loopback` in every build, so a
//! non-loopback peer is refused with 403 before bearer auth even runs.
//! `POST /shutdown` kills the process and `GET /auth` mints the key's
//! ambient cookie, so both sit here with the config surface they serve.
//!
//! Every bearer-authed handler in this tier extracts
//! [`crate::auth::LoopbackCaller`] rather than `AuthedCaller`: the wall is
//! the enforcement, the extractor is the handler's own statement of the
//! tier it belongs to, and a handler that is ever mounted without the wall
//! still refuses a LAN peer. The two `handoff` routes take no caller at
//! all: `/auth` is how a browser obtains its credential, and `/config`
//! only redirects.

#[cfg(feature = "local")]
pub(crate) mod chat_templates;
pub(crate) mod cloud_models;
pub(crate) mod config;
pub(crate) mod config_apply;
pub(crate) mod config_pending;
pub(crate) mod env_file;
// The browser handoff mints the config SPA's cookie, so the module
// exists only where that surface does; its build-independent auth
// primitives live in `crate::auth::primitives`.
#[cfg(feature = "config-ui")]
pub(crate) mod handoff;
pub(crate) mod hf;
#[cfg(feature = "local")]
pub(crate) mod model_info;
#[cfg(feature = "local")]
pub(crate) mod orphans;
pub(crate) mod reveal;
pub(crate) mod shutdown;
pub(crate) mod system;

use axum::Router;

use crate::AppState;
use crate::registry::RouteInfo;

/// The walled admin routes. The caller applies the loopback wall; this
/// router only assembles the tier, feature-gated areas included, so the
/// URL space in a build with a feature off is exactly the space with the
/// feature on minus that area.
pub(crate) fn routes() -> Router<AppState> {
    let router = Router::new()
        .merge(shutdown::routes())
        .merge(system::routes())
        .merge(config::routes())
        .merge(config_pending::routes())
        .merge(config_apply::routes())
        .merge(env_file::routes())
        .merge(cloud_models::routes())
        .merge(reveal::routes())
        .merge(hf::routes());
    // The template, orphan, and model-info routes read local-inference
    // facilities, so they exist only in builds with local inference.
    #[cfg(feature = "local")]
    let router = router
        .merge(chat_templates::routes())
        .merge(orphans::routes())
        .merge(model_info::routes());
    // The browser handoff onto the config SPA exists only when the SPA does.
    #[cfg(feature = "config-ui")]
    let router = router.merge(handoff::routes());
    router
}

/// The walled admin routes, as the registry sees them, under the same
/// feature gates [`routes`] mounts them.
pub(crate) fn registry() -> Vec<RouteInfo> {
    #[cfg_attr(
        not(any(feature = "local", feature = "config-ui")),
        expect(
            unused_mut,
            reason = "nothing extends the list when both gated groups are off"
        )
    )]
    let mut routes = [
        shutdown::ROUTES,
        system::ROUTES,
        config::ROUTES,
        config_pending::ROUTES,
        config_apply::ROUTES,
        env_file::ROUTES,
        cloud_models::ROUTES,
        reveal::ROUTES,
        hf::ROUTES,
    ]
    .concat();
    #[cfg(feature = "local")]
    routes.extend([chat_templates::ROUTES, orphans::ROUTES, model_info::ROUTES].concat());
    #[cfg(feature = "config-ui")]
    routes.extend_from_slice(handoff::ROUTES);
    routes
}
