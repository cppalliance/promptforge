//! The ambient-credential primitives [`check_auth`](super::check_auth)
//! reads: the handoff cookie's name, the session proof it holds, the
//! Fetch Metadata rules that gate ambient access, and the hex codec
//! between them.
//!
//! These exist in every build. The `/auth` route that mints the cookie
//! ships only with the config surface, but the cookie it minted is
//! accepted wherever bearer auth is, and the Fetch Metadata rules also
//! gate keyless loopback trust, which has no config surface at all. So
//! they sit beside the auth rules that read them rather than inside the
//! walled route module that writes them.
//!
//! The cookie never holds the key. Cookies are not port-isolated (RFC
//! 6265), so every local server the browser visits on the same address
//! receives them, and a cookie holding the key would hand any local
//! process the gateway discovery file's long-term secret on a single
//! navigation.
//! The value is instead the hex of a session proof - SHA-256 over a
//! process-lifetime random salt and the live key - so a harvested cookie
//! authenticates only until a restart or key rotation and reveals
//! nothing.

use axum::http::header::COOKIE;
use axum::http::{HeaderMap, HeaderName};

/// The cookie holding the session proof for browser sessions.
pub(crate) const AUTH_COOKIE: &str = "promptforge-gateway-session";

/// The `Sec-Fetch-Site` header name; the locked `http` crate has no
/// constant for it.
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// The one-time browser handoff URL for opening the config SPA:
/// `GET /auth` validates the key, sets the session cookie, and redirects to
/// the key-free `/config/`, so the key never sits in browser history. The
/// tray's Settings item, the relaunch handoff, and `--print-url` all build
/// their URL here. The key is percent-encoded: a generated key is hex and
/// passes through unchanged, but a configured key can contain query-special
/// characters (`/auth` decodes through serde_urlencoded).
pub(crate) fn auth_url(base_url: &str, key: &str) -> String {
    let key: String = url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
    format!("{base_url}/auth?key={key}")
}

/// Reads the handoff cookie's presented session proof, when the request
/// includes a well-formed one.
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

/// The session proof the cookie holds for `key` under `salt`: SHA-256
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
/// cross-origin page on another loopback port could otherwise use it
/// to reach state-changing routes: `SameSite=Lax` does not cover same-site
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
/// send `Sec-Fetch-Site`, and they are who keyless loopback is
/// for. A browser always sends it, so `cross-site` and `same-site` - a
/// page on any other origin using the user's loopback peer to reach
/// `POST /admin/shutdown` - are refused, as is any value the header
/// grammar does not name. `same-origin` (the config SPA) and `none` (a
/// typed URL) pass.
pub(crate) fn fetch_metadata_allows_ambient(headers: &HeaderMap) -> bool {
    match headers.get(SEC_FETCH_SITE) {
        None => true,
        Some(value) => matches!(value.to_str(), Ok("same-origin" | "none")),
    }
}

/// Hex-decodes a cookie value back to the presented key; `None` when the
/// value is not well-formed hex.
pub(crate) fn hex_decode(value: &str) -> Option<Vec<u8>> {
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

#[cfg(test)]
#[path = "auth-primitives-tests.rs"]
mod tests;
