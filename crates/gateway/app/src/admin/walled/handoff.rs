//! The browser handoff: `GET /auth?key=` validates the bearer key, sets a
//! session proof as an HttpOnly cookie, and redirects to the key-free
//! config UI URL, so the tray and shell can open the config surface in a
//! browser without leaving the bearer key in the browser's history.
//!
//! The whole module ships with the config surface it fronts. The cookie
//! it mints outlives that gate - [`crate::auth::check_auth`] accepts it in
//! every build as an alternative to the `Authorization` header - so the
//! cookie's name, its session proof, and the Fetch Metadata rules that
//! admit it are defined in [`crate::auth::primitives`], and only the
//! minting routes are here.
//!
//! `SameSite=Lax` keeps the cookie off cross-site requests, and the
//! loopback host wall keeps a rebound hostname from reaching the surface
//! at all. The cookie never contains the key itself; the proof is ambient,
//! so [`crate::auth::check_auth`] accepts it only with Fetch Metadata a
//! cross-origin page cannot strip: `SameSite=Lax` does not cover same-site
//! requests, since ports are not part of a site.

use axum::Router;
use axum::extract::{Query, State};
use axum::http::Method;
use axum::http::StatusCode;
use axum::http::header::{CACHE_CONTROL, LOCATION, SET_COOKIE};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;

use crate::AppState;
use crate::auth::primitives::{AUTH_COOKIE, session_token};
use crate::error::GatewayError;
use crate::registry::RouteInfo;

const CONFIG_REDIRECT: RouteInfo = RouteInfo::walled("/config", &[Method::GET]);
const AUTH: RouteInfo = RouteInfo::walled("/auth", &[Method::GET]);

/// The browser-entry routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[CONFIG_REDIRECT, AUTH];

/// The browser entry onto the config SPA: the `/auth` handoff and the
/// `/config` redirect onto the SPA mount. Neither takes an auth extractor:
/// the handoff is how the browser obtains its credential, and the
/// redirect sets nothing but a location. Both sit in the walled tier because
/// they exist only for the surface the wall protects.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(CONFIG_REDIRECT.path, get(config_ui_redirect))
        .route(AUTH.path, get(auth_handoff))
}

/// Redirects `GET /config` to `/config/`, where the SPA index is served
/// and its relative asset references resolve.
async fn config_ui_redirect() -> Redirect {
    Redirect::permanent("/config/")
}

/// Hex-encodes bytes for the cookie value: cookie-safe by construction.
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The `GET /auth?key=` query: the presented bearer key.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct AuthQuery {
    key: Option<String>,
}

/// The `GET /auth` browser-handoff route, walled loopback-only like the
/// config surface it fronts.
///
/// A wrong or missing key answers `401 Unauthorized` indistinguishably.
/// The right key answers `302 Found` to `/config/` - a clean, key-free
/// URL - with the key's session proof set as an HttpOnly,
/// `SameSite=Lax` session cookie and `Cache-Control: no-store` so the
/// handoff response itself is never reused from cache.
pub(crate) async fn auth_handoff(
    State(state): State<AppState>,
    Query(query): Query<AuthQuery>,
) -> Result<Response, GatewayError> {
    let live = state.live.read().await;
    let presented = query.key.unwrap_or_default();
    if !crate::auth::secret_eq(presented.as_bytes(), live.key.expose().as_bytes()) {
        return Err(GatewayError::Unauthorized);
    }
    let cookie = format!(
        "{AUTH_COOKIE}={}; HttpOnly; SameSite=Lax; Path=/",
        hex_encode(&session_token(
            &state.handoff_salt,
            live.key.expose().as_bytes()
        ))
    );
    drop(live);
    Ok((
        StatusCode::FOUND,
        [
            (LOCATION, String::from("/config/")),
            (SET_COOKIE, cookie),
            (CACHE_CONTROL, String::from("no-store")),
        ],
    )
        .into_response())
}

#[cfg(all(test, feature = "config-ui"))]
#[path = "handoff-tests.rs"]
mod tests;
