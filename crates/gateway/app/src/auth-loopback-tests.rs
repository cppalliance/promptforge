//! [`LoopbackCaller`] on its own, through a one-route router with no wall
//! in front of it: the extractor refuses a LAN or peerless caller with the
//! wall's bare 403 before auth runs, and admits a loopback caller only
//! when [`check_auth`] does.

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::AUTHORIZATION;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use tower::ServiceExt;

use super::LoopbackCaller;
use crate::AppState;
use crate::test_support::loopback_state;

const LOOPBACK: &str = "127.0.0.1:50000";
const LAN: &str = "198.51.100.7:44821";

async fn walled_handler(_caller: LoopbackCaller) -> StatusCode {
    StatusCode::NO_CONTENT
}

/// A router mounting the handler with no loopback wall, so the extractor
/// is the only thing standing between the caller and the handler.
fn unwalled_router(state: AppState) -> Router {
    Router::new()
        .route("/walled", get(walled_handler))
        .with_state(state)
}

async fn send(state: &AppState, peer: Option<&str>, authorization: Option<&str>) -> StatusCode {
    let mut builder = Request::builder().uri("/walled");
    if let Some(authorization) = authorization {
        builder = builder.header(AUTHORIZATION, authorization);
    }
    let mut request = builder
        .body(Body::empty())
        .expect("static request parts are valid");
    if let Some(peer) = peer {
        let peer: SocketAddr = peer.parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
    }
    unwalled_router(state.clone())
        .oneshot(request)
        .await
        .expect("the router is infallible")
        .status()
}

#[tokio::test]
async fn a_loopback_peer_with_the_key_is_admitted() {
    let state = loopback_state(Some(false));
    assert_eq!(
        send(&state, Some(LOOPBACK), Some("Bearer test-token")).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_lan_peer_is_refused_with_the_walls_403_even_with_the_key() {
    let state = loopback_state(Some(false));
    assert_eq!(
        send(&state, Some(LAN), Some("Bearer test-token")).await,
        StatusCode::FORBIDDEN,
        "the tier check comes before auth and does not care about the credential"
    );
}

#[tokio::test]
async fn a_peerless_request_fails_closed_with_403() {
    let state = loopback_state(Some(false));
    assert_eq!(
        send(&state, None, Some("Bearer test-token")).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn a_loopback_peer_without_a_credential_is_refused_by_auth_under_strict_mode() {
    let state = loopback_state(Some(false));
    assert_eq!(
        send(&state, Some(LOOPBACK), None).await,
        StatusCode::UNAUTHORIZED,
        "past the tier check, the ordinary auth rules decide"
    );
}

#[tokio::test]
async fn a_loopback_peer_without_a_credential_is_admitted_under_loopback_trust() {
    let state = loopback_state(Some(true));
    assert_eq!(
        send(&state, Some(LOOPBACK), None).await,
        StatusCode::NO_CONTENT
    );
}
