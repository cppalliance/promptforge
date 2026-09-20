//! The browser handoff: `GET /auth?key=` validates the bearer key, sets a
//! session proof as an HttpOnly cookie, and redirects to the key-free
//! config UI URL, so the tray and shell can open the config surface in a
//! browser without leaving the bearer key in the browser's history.
//!
//! The cookie is the key's ambient form: [`crate::auth::check_auth`] accepts it
//! in every build as an alternative to the `Authorization` header, so the
//! SPA the redirect lands on can call the admin surface without ever
//! seeing the key. `SameSite=Lax` keeps the cookie off cross-site
//! requests, and the loopback host wall keeps a rebound hostname from
//! reaching the surface at all.
//!
//! The cookie never carries the key. Cookies are not port-isolated (RFC
//! 6265), so every local server the browser visits on the same address
//! receives them, and a key-carrying cookie would hand any local process
//! the gateway discovery file's long-term secret on a single navigation. The
//! value is instead the hex of a session proof - SHA-256 over a
//! process-lifetime random salt and the live key - so a harvested cookie
//! authenticates only until a restart or key rotation and reveals nothing.
//! And because the proof is ambient, [`crate::auth::check_auth`] accepts it only
//! with Fetch Metadata a cross-origin page cannot strip: `SameSite=Lax`
//! does not cover same-site requests, since ports are not part of a site.

#[cfg(feature = "config-ui")]
use axum::Router;
#[cfg(feature = "config-ui")]
use axum::extract::{Query, State};
#[cfg(feature = "config-ui")]
use axum::http::Method;
#[cfg(feature = "config-ui")]
use axum::http::StatusCode;
use axum::http::header::COOKIE;
#[cfg(feature = "config-ui")]
use axum::http::header::{CACHE_CONTROL, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderName};
#[cfg(feature = "config-ui")]
use axum::response::{IntoResponse, Redirect, Response};
#[cfg(feature = "config-ui")]
use axum::routing::get;

#[cfg(feature = "config-ui")]
use crate::AppState;
#[cfg(feature = "config-ui")]
use crate::error::GatewayError;
#[cfg(feature = "config-ui")]
use crate::registry::RouteInfo;

#[cfg(feature = "config-ui")]
const CONFIG_REDIRECT: RouteInfo = RouteInfo::walled("/config", &[Method::GET]);
#[cfg(feature = "config-ui")]
const AUTH: RouteInfo = RouteInfo::walled("/auth", &[Method::GET]);

/// The browser-entry routes, as the registry sees them.
#[cfg(feature = "config-ui")]
pub(crate) const ROUTES: &[RouteInfo] = &[CONFIG_REDIRECT, AUTH];

/// The browser entry onto the config SPA: the `/auth` handoff and the
/// `/config` redirect onto the SPA mount. Neither takes an auth extractor:
/// the handoff is how the browser earns its credential, and the redirect
/// carries nothing but a location. Both sit in the walled tier because
/// they exist only for the surface the wall protects.
#[cfg(feature = "config-ui")]
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(CONFIG_REDIRECT.path, get(config_ui_redirect))
        .route(AUTH.path, get(auth_handoff))
}

/// Redirects `GET /config` to `/config/`, where the SPA index is served
/// and its relative asset references resolve.
#[cfg(feature = "config-ui")]
async fn config_ui_redirect() -> Redirect {
    Redirect::permanent("/config/")
}

/// The cookie carrying the session proof for browser sessions.
pub(crate) const AUTH_COOKIE: &str = "promptforge-gateway-session";

/// The one-time browser handoff URL for opening the config SPA:
/// `GET /auth` validates the key, sets the session cookie, and redirects to
/// the key-free `/config/`, so the key never sits in browser history. The
/// tray's Settings item, the relaunch handoff, and `--print-url` all build
/// their URL here. The key is percent-encoded: a generated key is hex and
/// passes through unchanged, but a configured key can carry query-special
/// characters (`/auth` decodes through serde_urlencoded).
pub(crate) fn auth_url(base_url: &str, key: &str) -> String {
    let key: String = url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
    format!("{base_url}/auth?key={key}")
}

/// The `Sec-Fetch-Site` header name; the locked `http` crate carries no
/// constant for it.
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// Reads the handoff cookie's presented session proof, when the request
/// carries a well-formed one.
pub(crate) fn presented_cookie_proof(headers: &HeaderMap) -> Option<Vec<u8>> {
    let header = headers.get(COOKIE)?.to_str().ok()?;
    header.split(';').map(str::trim).find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name == AUTH_COOKIE {
            hex_decode(value)
        } else {
            None
        }
    })
}

/// The session proof the cookie carries for `key` under `salt`: SHA-256
/// over the process-lifetime salt and the live key. The proof, never the
/// key, crosses into the browser, so a harvested cookie authenticates only
/// until a restart or key rotation and reveals nothing about the key.
pub(crate) fn session_token(salt: &[u8; 32], key: &[u8]) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    let mut digest = Sha256::new();
    digest.update(salt);
    digest.update(key);
    digest.finalize().into()
}

/// Whether the request's Fetch Metadata permits cookie authentication.
/// The cookie is ambient - no `Authorization` header to require - so a
/// cross-origin page on another loopback port could otherwise ride it
/// into state-changing routes: `SameSite=Lax` does not cover same-site
/// requests, since ports are not part of a site. Every supported browser
/// attaches `Sec-Fetch-Site` to page-initiated requests, and a page
/// cannot strip or forge it; bearer clients (the shell, the tray,
/// scripts) never take the cookie path.
pub(crate) fn fetch_metadata_allows_cookie(headers: &HeaderMap) -> bool {
    matches!(
        headers
            .get(SEC_FETCH_SITE)
            .and_then(|value| value.to_str().ok()),
        Some("same-origin" | "none")
    )
}

/// Whether the request's Fetch Metadata permits ambient, credential-free
/// access from a loopback peer. Unlike the cookie rule, an absent header
/// is admitted: non-browser clients (curl, the SDK, the workshop) never
/// send `Sec-Fetch-Site`, and they are exactly who keyless loopback is
/// for. A browser always sends it, so `cross-site` and `same-site` - a
/// page on any other origin riding the user's loopback peer into
/// `POST /admin/shutdown` - are refused, as is any value the header
/// grammar does not name. `same-origin` (the config SPA) and `none` (a
/// typed URL) pass.
pub(crate) fn fetch_metadata_allows_ambient(headers: &HeaderMap) -> bool {
    match headers.get(SEC_FETCH_SITE) {
        None => true,
        Some(value) => matches!(value.to_str(), Ok("same-origin" | "none")),
    }
}

/// Hex-encodes bytes for the cookie value: cookie-safe by construction.
#[cfg(feature = "config-ui")]
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Hex-decodes a cookie value back to the presented key; `None` when the
/// value is not well-formed hex.
fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let hi = hex_digit(pair[0])?;
        let lo = hex_digit(pair[1])?;
        out.push(hi << 4 | lo);
    }
    Some(out)
}

/// One lowercase-or-uppercase ASCII hex digit's value.
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// The `GET /auth?key=` query: the presented bearer key.
#[cfg(feature = "config-ui")]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct AuthQuery {
    key: Option<String>,
}

/// The `GET /auth` browser-handoff route, walled loopback-only like the
/// config surface it fronts.
///
/// A wrong or missing key answers `401 Unauthorized` indistinguishably.
/// The right key answers `302 Found` to `/config/` - a clean URL carrying
/// no key - with the key's session proof set as an HttpOnly,
/// `SameSite=Lax` session cookie and `Cache-Control: no-store` so the
/// handoff response itself is never reused from cache.
#[cfg(feature = "config-ui")]
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

#[cfg(test)]
#[path = "handoff-cookie-tests.rs"]
mod cookie_tests;
