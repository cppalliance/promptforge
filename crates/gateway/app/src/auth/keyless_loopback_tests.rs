//! Rule 3 of [`check_auth`] through the real router: a
//! credential-free loopback caller is admitted on every route class
//! unless its Fetch Metadata marks a cross-origin page, a wrong
//! bearer is still refused on loopback, a LAN or peerless caller
//! receives no trust, and `trust_loopback = false` restores strict
//! bearer auth.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use crate::test_support::loopback_state;
use crate::{AppState, build_router};

const LOOPBACK: &str = "127.0.0.1:50000";
const LAN: &str = "198.51.100.7:44821";

/// One route from each class - the inference surface, the any-source
/// admin surface, and the walled shutdown route - with the status an
/// admitted empty-bodied request receives on it.
fn routes() -> Vec<(Method, &'static str, StatusCode)> {
    vec![
        (Method::GET, "/v1/models", StatusCode::OK),
        (Method::GET, "/admin/status", StatusCode::OK),
        (Method::POST, "/shutdown", StatusCode::ACCEPTED),
    ]
}

/// Sends one empty-bodied request with the given peer planted (or
/// none), an optional `Authorization` header value, and an optional
/// `Sec-Fetch-Site` value.
async fn send(
    state: &AppState,
    method: Method,
    path: &str,
    peer: Option<&str>,
    authorization: Option<&str>,
    sec_fetch_site: Option<&str>,
) -> StatusCode {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(authorization) = authorization {
        builder = builder.header(AUTHORIZATION, authorization);
    }
    if let Some(site) = sec_fetch_site {
        builder = builder.header("sec-fetch-site", site);
    }
    let mut request = builder
        .body(Body::empty())
        .expect("static request parts are valid");
    if let Some(peer) = peer {
        let peer: SocketAddr = peer.parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
    }
    build_router(state.clone(), None)
        .oneshot(request)
        .await
        .expect("the router is infallible")
        .status()
}

#[tokio::test]
async fn a_keyless_loopback_caller_is_admitted_on_every_route_class() {
    let state = loopback_state(None);
    for (method, path, admitted) in routes() {
        let status = send(&state, method.clone(), path, Some(LOOPBACK), None, None).await;
        assert_eq!(
            status, admitted,
            "{method} {path}: a loopback peer with no credential and no fetch metadata is admitted"
        );
    }
}

#[tokio::test]
async fn same_origin_and_none_fetch_metadata_keep_the_keyless_loopback_caller_admitted() {
    let state = loopback_state(None);
    for site in ["same-origin", "none"] {
        for (method, path, admitted) in routes() {
            let status = send(
                &state,
                method.clone(),
                path,
                Some(LOOPBACK),
                None,
                Some(site),
            )
            .await;
            assert_eq!(
                status, admitted,
                "{method} {path}: Sec-Fetch-Site: {site} is the SPA or a typed URL"
            );
        }
    }
}

#[tokio::test]
async fn cross_origin_fetch_metadata_refuses_the_keyless_loopback_caller() {
    let state = loopback_state(None);
    for site in ["cross-site", "same-site", "not-a-site"] {
        for (method, path, _admitted) in routes() {
            let status = send(
                &state,
                method.clone(),
                path,
                Some(LOOPBACK),
                None,
                Some(site),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {path}: Sec-Fetch-Site: {site} marks a page request from the loopback peer"
            );
        }
    }
}

/// The CSRF shape the rule exists to stop, on the inference route a
/// page would actually target: a well-formed chat request from a
/// loopback peer is refused when `Sec-Fetch-Site: cross-site` marks it
/// as another origin's page, and reaches routing (404: no such model)
/// when the same request includes no fetch metadata.
#[tokio::test]
async fn cross_site_fetch_metadata_refuses_a_keyless_loopback_chat_completion() {
    let state = loopback_state(None);
    let chat = |site: Option<&'static str>| {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/v1/chat/completions")
            .header("content-type", "application/json");
        if let Some(site) = site {
            builder = builder.header("sec-fetch-site", site);
        }
        let mut request = builder
            .body(Body::from(
                r#"{"model":"no-such-model","messages":[{"role":"user","content":"ping"}]}"#,
            ))
            .expect("static request parts are valid");
        let peer: SocketAddr = LOOPBACK.parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    };
    let router = build_router(state.clone(), None);

    let refused = router
        .clone()
        .oneshot(chat(Some("cross-site")))
        .await
        .expect("the router is infallible")
        .status();
    assert_eq!(
        refused,
        StatusCode::UNAUTHORIZED,
        "a cross-site page request from the loopback peer never reaches routing"
    );

    let admitted = router
        .oneshot(chat(None))
        .await
        .expect("the router is infallible")
        .status();
    assert_eq!(
        admitted,
        StatusCode::NOT_FOUND,
        "the same request with no fetch metadata passes auth and is judged by routing"
    );
}

#[tokio::test]
async fn a_wrong_bearer_on_loopback_stays_401() {
    let state = loopback_state(None);
    for (method, path, _admitted) in routes() {
        let status = send(
            &state,
            method.clone(),
            path,
            Some(LOOPBACK),
            Some("Bearer wrong"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path}: presenting a wrong credential is not the same as presenting none"
        );
    }
}

#[tokio::test]
async fn a_keyless_lan_peer_is_refused_everywhere() {
    let state = loopback_state(None);
    for (method, path, _admitted) in routes() {
        let status = send(&state, method.clone(), path, Some(LAN), None, None).await;
        // The walled shutdown route refuses a LAN peer before auth runs.
        let refused = if path == "/shutdown" {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::UNAUTHORIZED
        };
        assert_eq!(
            status, refused,
            "{method} {path}: loopback trust never reaches a LAN peer"
        );
    }
}

#[tokio::test]
async fn a_keyless_caller_with_no_peer_address_is_refused_everywhere() {
    let state = loopback_state(None);
    for (method, path, _admitted) in routes() {
        let status = send(&state, method.clone(), path, None, None, None).await;
        let refused = if path == "/shutdown" {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::UNAUTHORIZED
        };
        assert_eq!(
            status, refused,
            "{method} {path}: no recorded peer means no trust"
        );
    }
}

#[tokio::test]
async fn trust_loopback_false_requires_the_key_from_a_loopback_peer() {
    let state = loopback_state(Some(false));
    for (method, path, admitted) in routes() {
        let keyless = send(&state, method.clone(), path, Some(LOOPBACK), None, None).await;
        assert_eq!(
            keyless,
            StatusCode::UNAUTHORIZED,
            "{method} {path}: the opt-out restores strict bearer auth"
        );
        let keyed = send(
            &state,
            method.clone(),
            path,
            Some(LOOPBACK),
            Some("Bearer test-token"),
            None,
        )
        .await;
        assert_eq!(
            keyed, admitted,
            "{method} {path}: the key still admits under the opt-out"
        );
    }
}

#[tokio::test]
async fn an_explicit_trust_loopback_true_matches_the_default() {
    let state = loopback_state(Some(true));
    let status = send(
        &state,
        Method::GET,
        "/admin/status",
        Some(LOOPBACK),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}
