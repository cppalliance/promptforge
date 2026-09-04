//! The browser handoff: `GET /auth?key=` validates the bearer key, sets a
//! session proof as an HttpOnly cookie, and redirects to the key-free
//! config UI URL, so the tray and shell can open the config surface in a
//! browser without leaving the bearer key in the browser's history.
//!
//! The cookie is the key's ambient form: [`crate::check_auth`] accepts it
//! in every build as an alternative to the `Authorization` header, so the
//! SPA the redirect lands on can call the admin surface without ever
//! seeing the key. `SameSite=Lax` keeps the cookie off cross-site
//! requests, and the loopback host wall keeps a rebound hostname from
//! reaching the surface at all.
//!
//! The cookie never carries the key. Cookies are not port-isolated (RFC
//! 6265), so every local server the browser visits on the same address
//! receives them, and a key-carrying cookie would hand any local process
//! the connection file's long-term secret on a single navigation. The
//! value is instead the hex of a session proof - SHA-256 over a
//! process-lifetime random salt and the live key - so a harvested cookie
//! authenticates only until a restart or key rotation and reveals nothing.
//! And because the proof is ambient, [`crate::check_auth`] accepts it only
//! with Fetch Metadata a cross-origin page cannot strip: `SameSite=Lax`
//! does not cover same-site requests, since ports are not part of a site.

#[cfg(feature = "config-ui")]
use axum::extract::{Query, State};
#[cfg(feature = "config-ui")]
use axum::http::StatusCode;
use axum::http::header::COOKIE;
#[cfg(feature = "config-ui")]
use axum::http::header::{CACHE_CONTROL, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, HeaderName};
#[cfg(feature = "config-ui")]
use axum::response::{IntoResponse, Response};

#[cfg(feature = "config-ui")]
use crate::AppState;
#[cfg(feature = "config-ui")]
use crate::error::GatewayError;

/// The cookie carrying the session proof for browser sessions.
pub(crate) const AUTH_COOKIE: &str = "promptforge-gateway-session";

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
    if value.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
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
    if !crate::secret_eq(presented.as_bytes(), live.key.expose().as_bytes()) {
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
mod tests {
    // The compound cfg hides the module from clippy's test detection, so
    // the test-code expect/unwrap allowance is restated explicitly.
    #![expect(
        clippy::expect_used,
        reason = "the shared test fixture fails with the invariant named"
    )]

    use std::net::SocketAddr;

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::header::{CACHE_CONTROL, LOCATION, SET_COOKIE};
    use axum::http::{Request, Response, StatusCode};
    use gateway_config::Config;
    use tower::ServiceExt;

    use super::{hex_encode, session_token};
    use crate::test_support::app_state;
    use crate::{AppState, build_router};

    fn state() -> AppState {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
        )
        .expect("config parses");
        app_state(config, None)
    }

    /// Sends one `GET /auth...` through the router with a loopback peer
    /// planted, as the walled route requires.
    async fn get_auth(state: &AppState, uri: &str) -> Response<Body> {
        let mut request = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("request builds");
        let peer: SocketAddr = "127.0.0.1:50000".parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
        build_router(state.clone(), None)
            .oneshot(request)
            .await
            .expect("the router is infallible")
    }

    #[tokio::test]
    async fn a_wrong_or_missing_key_is_rejected_with_401() {
        let state = state();
        for uri in ["/auth?key=wrong", "/auth", "/auth?key="] {
            let response = get_auth(&state, uri).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }
    }

    #[tokio::test]
    async fn the_right_key_sets_the_cookie_and_redirects_key_free() {
        let state = state();
        let response = get_auth(&state, "/auth?key=test-token").await;
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers().get(LOCATION).expect("a Location header"),
            "/config/",
            "the redirect target carries no key"
        );
        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .expect("a Set-Cookie header")
            .to_str()
            .expect("the cookie is header-safe");
        assert!(
            cookie.starts_with("promptforge-gateway-session="),
            "the handoff cookie: {cookie}"
        );
        let value = cookie
            .split(';')
            .next()
            .and_then(|pair| pair.split_once('='))
            .map(|(_, value)| value)
            .expect("the cookie carries a value");
        assert_eq!(
            value,
            hex_encode(&session_token(&state.handoff_salt, b"test-token")),
            "the cookie carries the session proof, never the key: {cookie}"
        );
        assert!(
            cookie.contains("HttpOnly"),
            "the cookie is HttpOnly: {cookie}"
        );
        assert!(
            cookie.contains("SameSite=Lax"),
            "the cookie is SameSite=Lax: {cookie}"
        );
        assert!(
            !cookie.contains("test-token"),
            "the cookie never carries the raw key: {cookie}"
        );
        assert_eq!(
            response
                .headers()
                .get(CACHE_CONTROL)
                .expect("a Cache-Control header"),
            "no-store",
            "the handoff response is never cached"
        );
    }

    #[tokio::test]
    async fn the_route_refuses_a_lan_peer_even_with_the_key() {
        let state = state();
        let mut request = Request::builder()
            .uri("/auth?key=test-token")
            .body(Body::empty())
            .expect("request builds");
        let peer: SocketAddr = "198.51.100.7:44821".parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
        let response = build_router(state, None)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}

#[cfg(test)]
mod cookie_tests {
    use axum::http::HeaderMap;
    use axum::http::header::{AUTHORIZATION, COOKIE};
    use gateway_config::Config;

    use super::{AUTH_COOKIE, SEC_FETCH_SITE, hex_decode, presented_cookie_proof, session_token};
    use crate::AppState;
    use crate::test_support::app_state;

    /// Hex-encodes as the route's `hex_encode` does; that encoder is
    /// compiled only with the config surface, while these cookie-auth
    /// tests run in every build.
    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// The cookie header value the `/auth` route mints for `state`'s
    /// salt and `key`, as the browser would present it back.
    fn minted_cookie(state: &AppState, key: &str) -> String {
        format!(
            "{AUTH_COOKIE}={}",
            hex(&session_token(&state.handoff_salt, key.as_bytes()))
        )
    }

    /// A state whose configured bearer key is `test-token`.
    fn test_token_state() -> AppState {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
        )
        .expect("config parses");
        app_state(config, None)
    }

    /// Headers presenting `cookie` from a same-origin browser page.
    fn same_origin_with(cookie: &str) -> HeaderMap {
        HeaderMap::from_iter([
            (COOKIE, cookie.parse().expect("a header value")),
            (
                SEC_FETCH_SITE,
                "same-origin".parse().expect("a header value"),
            ),
        ])
    }

    #[test]
    fn the_cookie_parses_among_others() {
        let headers = HeaderMap::from_iter([(
            COOKIE,
            format!("session=abc; {AUTH_COOKIE}=746573742d746f6b656e; theme=dark")
                .parse()
                .expect("a header value"),
        )]);
        assert_eq!(
            presented_cookie_proof(&headers).as_deref(),
            Some(b"test-token".as_slice())
        );
    }

    #[test]
    fn malformed_cookies_present_nothing() {
        for cookie in [
            "promptforge-gateway-session=zz",  // not hex
            "promptforge-gateway-session=abc", // odd length
            "other=746573742d746f6b656e",      // the wrong name
            "promptforge-gateway-session",     // no value at all
        ] {
            let headers = HeaderMap::from_iter([(COOKIE, cookie.parse().expect("a header value"))]);
            assert_eq!(presented_cookie_proof(&headers), None, "{cookie}");
        }
        assert_eq!(presented_cookie_proof(&HeaderMap::new()), None);
    }

    #[test]
    fn hex_decode_round_trips_through_the_encoder() {
        #[cfg(feature = "config-ui")]
        {
            let key = b"an arbitrary key/with+odd=chars";
            assert_eq!(
                hex_decode(&super::hex_encode(key)).as_deref(),
                Some(key.as_slice())
            );
        }
        assert_eq!(hex_decode("").as_deref(), Some(b"".as_slice()));
        assert_eq!(
            hex_decode("00ff40").as_deref(),
            Some(&[0x00, 0xff, 0x40][..])
        );
    }

    #[tokio::test]
    async fn check_auth_accepts_the_cookie_as_the_bearer_keys_ambient_form() {
        let state = test_token_state();
        let headers = same_origin_with(&minted_cookie(&state, "test-token"));
        assert!(crate::check_auth(&state, &headers).await.is_ok());

        // A wrong cookie and a wrong bearer both stay refused.
        let wrong = same_origin_with(&format!("{AUTH_COOKIE}={}", hex(b"wrong")));
        assert!(crate::check_auth(&state, &wrong).await.is_err());
        let both = HeaderMap::from_iter([
            (
                AUTHORIZATION,
                "Bearer wrong".parse().expect("a header value"),
            ),
            (
                COOKIE,
                minted_cookie(&state, "test-token")
                    .parse()
                    .expect("a header value"),
            ),
            (
                SEC_FETCH_SITE,
                "same-origin".parse().expect("a header value"),
            ),
        ]);
        assert!(
            crate::check_auth(&state, &both).await.is_ok(),
            "a valid cookie authenticates even alongside a wrong bearer header"
        );
    }

    #[tokio::test]
    async fn the_cookie_carries_a_session_proof_never_the_key() {
        let state = test_token_state();
        // The key's own hex - what a key-carrying cookie would present -
        // must not authenticate.
        let bare = same_origin_with(&format!("{AUTH_COOKIE}={}", hex(b"test-token")));
        assert!(
            crate::check_auth(&state, &bare).await.is_err(),
            "the cookie carries a derived proof, so the key itself is refused"
        );
        // A proof minted under another process's salt is refused: a
        // restart revokes every minted cookie.
        let foreign = same_origin_with(&format!(
            "{AUTH_COOKIE}={}",
            hex(&session_token(&[0xAB; 32], b"test-token"))
        ));
        assert!(
            crate::check_auth(&state, &foreign).await.is_err(),
            "a proof minted under another salt is refused"
        );
    }

    #[tokio::test]
    async fn the_cookie_path_requires_same_origin_fetch_metadata() {
        let state = test_token_state();
        // A cross-origin rider on another loopback port is same-site
        // (ports are not part of a site), so SameSite does not stop it;
        // the fetch metadata it cannot strip does.
        for site in ["same-site", "cross-site"] {
            let headers = HeaderMap::from_iter([
                (
                    COOKIE,
                    minted_cookie(&state, "test-token")
                        .parse()
                        .expect("a header value"),
                ),
                (SEC_FETCH_SITE, site.parse().expect("a header value")),
            ]);
            assert!(
                crate::check_auth(&state, &headers).await.is_err(),
                "Sec-Fetch-Site: {site} marks a cross-origin rider"
            );
        }
        // No metadata at all: non-browser clients authenticate with the
        // bearer header, never the cookie.
        let bare = HeaderMap::from_iter([(
            COOKIE,
            minted_cookie(&state, "test-token")
                .parse()
                .expect("a header value"),
        )]);
        assert!(crate::check_auth(&state, &bare).await.is_err());
        // `none` is the user-driven navigation case and is admitted.
        let navigation = HeaderMap::from_iter([
            (
                COOKIE,
                minted_cookie(&state, "test-token")
                    .parse()
                    .expect("a header value"),
            ),
            (SEC_FETCH_SITE, "none".parse().expect("a header value")),
        ]);
        assert!(crate::check_auth(&state, &navigation).await.is_ok());
    }
}
