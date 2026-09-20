use axum::http::HeaderMap;
use axum::http::header::{AUTHORIZATION, COOKIE};
use gateway_config::Config;

use super::{
    AUTH_COOKIE, SEC_FETCH_SITE, auth_url, fetch_metadata_allows_ambient,
    fetch_metadata_allows_cookie, hex_decode, presented_cookie_proof, session_token,
};
use crate::AppState;
use crate::auth::Caller;
use crate::test_support::app_state;

/// A caller with no recorded peer address: the cookie rules are
/// exercised on their own, with loopback trust out of reach.
fn peerless(headers: HeaderMap) -> Caller {
    Caller::new(headers, None)
}

#[test]
fn the_auth_url_targets_the_one_time_handoff() {
    assert_eq!(
        auth_url("http://127.0.0.1:8081", "abc123"),
        "http://127.0.0.1:8081/auth?key=abc123"
    );
}

#[test]
fn the_auth_url_percent_encodes_a_configured_key() {
    // The WHATWG urlencoded byte serializer encodes space as `+`;
    // serde_urlencoded decodes it back.
    assert_eq!(
        auth_url("http://127.0.0.1:8081", "a&b=c d"),
        "http://127.0.0.1:8081/auth?key=a%26b%3Dc+d",
        "a configured key with query-special characters survives the handoff"
    );
}

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
        "config-version = 0\n\
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
fn ambient_fetch_metadata_admits_absent_same_origin_and_none_only() {
    let with = |site: &str| {
        HeaderMap::from_iter([(SEC_FETCH_SITE, site.parse().expect("a header value"))])
    };
    assert!(
        fetch_metadata_allows_ambient(&HeaderMap::new()),
        "a non-browser client sends no Sec-Fetch-Site"
    );
    assert!(
        !fetch_metadata_allows_cookie(&HeaderMap::new()),
        "the cookie rule stays strict: absent metadata is refused there"
    );
    for site in ["same-origin", "none"] {
        assert!(fetch_metadata_allows_ambient(&with(site)), "{site}");
    }
    for site in ["cross-site", "same-site", "garbage", ""] {
        assert!(!fetch_metadata_allows_ambient(&with(site)), "{site:?}");
    }
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
    assert!(
        crate::auth::check_auth(&state, &peerless(headers))
            .await
            .is_ok()
    );

    // A wrong cookie and a wrong bearer both stay refused.
    let wrong = same_origin_with(&format!("{AUTH_COOKIE}={}", hex(b"wrong")));
    assert!(
        crate::auth::check_auth(&state, &peerless(wrong))
            .await
            .is_err()
    );
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
        crate::auth::check_auth(&state, &peerless(both))
            .await
            .is_ok(),
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
        crate::auth::check_auth(&state, &peerless(bare))
            .await
            .is_err(),
        "the cookie carries a derived proof, so the key itself is refused"
    );
    // A proof minted under another process's salt is refused: a
    // restart revokes every minted cookie.
    let foreign = same_origin_with(&format!(
        "{AUTH_COOKIE}={}",
        hex(&session_token(&[0xAB; 32], b"test-token"))
    ));
    assert!(
        crate::auth::check_auth(&state, &peerless(foreign))
            .await
            .is_err(),
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
            crate::auth::check_auth(&state, &peerless(headers))
                .await
                .is_err(),
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
    assert!(
        crate::auth::check_auth(&state, &peerless(bare))
            .await
            .is_err()
    );
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
    assert!(
        crate::auth::check_auth(&state, &peerless(navigation))
            .await
            .is_ok()
    );
}
