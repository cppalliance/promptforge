//! The route registry: every mounted route as data, with the tier that
//! mounts it.
//!
//! Each area module declares one [`RouteInfo`] constant per route and
//! binds `INFO.path` in its `routes()`, never a literal, so the registry
//! and the router cannot name different paths. [`all`] collects every
//! area's `ROUTES` under the same feature gates `build_router` merges them
//! under. The registry is the crate's one enumerable route list: the
//! tests sweep it to prove every walled route refuses a LAN peer and every
//! open route does not, and a route declared in the wrong tier fails that
//! sweep rather than a reader's memory.

use axum::http::Method;

use crate::admin;
#[cfg(feature = "local")]
use crate::cache;
#[cfg(feature = "web-search")]
use crate::web_search;
use crate::{health, models, relay, speech};

/// Which tier mounts a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    /// Bearer-authed, reachable from any peer the listener admits.
    Open,
    /// Behind the shared loopback wall: a non-loopback peer receives 403
    /// before auth runs.
    Walled,
}

/// One mounted route: its axum path template, the methods it answers,
/// and the tier that mounts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RouteInfo {
    /// The path template as `Router::route` receives it, captures in
    /// braces.
    pub(crate) path: &'static str,
    /// The methods mounted on the path.
    pub(crate) methods: &'static [Method],
    /// The tier that mounts the route.
    pub(crate) tier: Tier,
}

impl RouteInfo {
    /// A route in the open tier.
    pub(crate) const fn open(path: &'static str, methods: &'static [Method]) -> RouteInfo {
        RouteInfo {
            path,
            methods,
            tier: Tier::Open,
        }
    }

    /// A route in the walled tier.
    pub(crate) const fn walled(path: &'static str, methods: &'static [Method]) -> RouteInfo {
        RouteInfo {
            path,
            methods,
            tier: Tier::Walled,
        }
    }
}

/// Every route the assembled router mounts in this build, in mount order.
pub(crate) fn all() -> Vec<RouteInfo> {
    let mut routes = Vec::new();
    routes.extend_from_slice(relay::ROUTES);
    routes.extend_from_slice(speech::ROUTES);
    routes.extend_from_slice(models::ROUTES);
    routes.extend_from_slice(health::ROUTES);
    routes.extend(admin::open::registry());
    #[cfg(feature = "web-search")]
    routes.extend_from_slice(web_search::ROUTES);
    #[cfg(feature = "local")]
    routes.extend_from_slice(cache::ROUTES);
    routes.extend(admin::walled::registry());
    routes
}

#[cfg(test)]
#[path = "registry-tests.rs"]
mod tests;
