//! The registry against the assembled router: every route the registry
//! declares walled refuses a LAN peer and a peerless caller with the
//! wall's 403 and admits a loopback peer past it; every route it declares
//! open is mounted and answers a LAN peer with something other than 403.
//! Because the wall only runs on a matched route, a 403 from the LAN is
//! also proof the route is mounted, so a registry entry naming a path no
//! module mounts fails here, as does a route mounted in the wrong tier.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use super::{RouteInfo, Tier, all};
use crate::test_support::walled_fixture;
use crate::{AppState, build_router};

const LOOPBACK: &str = "127.0.0.1:50000";
const LAN: &str = "198.51.100.7:44821";

/// A concrete request path for a registry template: each capture is
/// filled with a value that fails the handler's own validation, so a
/// sweep that reaches the handler is refused there and never does real
/// work (the HF repo `owner/na me` fails the repo check; the cache digest
/// is not hex).
fn concrete_path(template: &str) -> String {
    template
        .replace("{owner}", "owner")
        .replace("{name}", "na%20me")
        .replace("{sha256}", "not-a-digest")
}

/// Sends one empty-bodied request through `build_router` with the valid
/// bearer key and the given peer planted as `ConnectInfo` (or none).
async fn send(state: &AppState, method: &Method, path: &str, peer: Option<&str>) -> StatusCode {
    let mut request = Request::builder()
        .method(method.clone())
        .uri(path)
        .header(AUTHORIZATION, "Bearer test-token")
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

fn routes_in(tier: Tier) -> Vec<RouteInfo> {
    all()
        .into_iter()
        .filter(|route| route.tier == tier)
        .collect()
}

#[test]
fn the_registry_names_every_path_once_and_both_tiers() {
    let routes = all();
    let mut paths: Vec<&str> = routes.iter().map(|route| route.path).collect();
    paths.sort_unstable();
    let before = paths.len();
    paths.dedup();
    assert_eq!(
        before,
        paths.len(),
        "a path is declared by exactly one module"
    );
    assert!(
        routes.iter().any(|route| route.tier == Tier::Open),
        "the open tier is non-empty"
    );
    assert!(
        routes.iter().any(|route| route.tier == Tier::Walled),
        "the walled tier is non-empty"
    );
    for route in &routes {
        assert!(
            !route.methods.is_empty(),
            "{}: every route declares at least one method",
            route.path
        );
    }
}

#[tokio::test]
async fn every_walled_route_refuses_a_lan_peer_with_403() {
    let (_temp, state) = walled_fixture();
    for route in routes_in(Tier::Walled) {
        let path = concrete_path(route.path);
        for method in route.methods {
            assert_eq!(
                send(&state, method, &path, Some(LAN)).await,
                StatusCode::FORBIDDEN,
                "{method} {path} is declared walled: it must refuse a LAN peer even with the valid key"
            );
        }
    }
}

#[tokio::test]
async fn every_walled_route_fails_closed_without_a_peer_address() {
    let (_temp, state) = walled_fixture();
    for route in routes_in(Tier::Walled) {
        let path = concrete_path(route.path);
        for method in route.methods {
            assert_eq!(
                send(&state, method, &path, None).await,
                StatusCode::FORBIDDEN,
                "{method} {path} is declared walled: it must fail closed with no peer address"
            );
        }
    }
}

#[tokio::test]
async fn every_walled_route_admits_a_loopback_peer_past_the_wall() {
    let (_temp, state) = walled_fixture();
    for route in routes_in(Tier::Walled) {
        let path = concrete_path(route.path);
        for method in route.methods {
            let status = send(&state, method, &path, Some(LOOPBACK)).await;
            assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} must pass the wall for a loopback peer"
            );
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {path} is declared but nothing mounts it"
            );
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is declared but the mount does not answer that method"
            );
        }
    }
}

#[tokio::test]
async fn every_open_route_is_mounted_and_reachable_from_the_lan() {
    let (_temp, state) = walled_fixture();
    for route in routes_in(Tier::Open) {
        let path = concrete_path(route.path);
        for method in route.methods {
            let status = send(&state, method, &path, Some(LAN)).await;
            assert_ne!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {path} is declared open: a LAN peer with the key must not hit a wall"
            );
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {path} is declared but nothing mounts it"
            );
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is declared but the mount does not answer that method"
            );
        }
    }
}
