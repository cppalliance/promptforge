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
        "config-version = 0\n\
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
