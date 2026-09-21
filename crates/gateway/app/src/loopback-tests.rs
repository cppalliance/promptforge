//! The host-authority wall and the config SPA mount through the real
//! router. The per-route loopback wall sweeps sit beside the registry
//! (`registry-tests.rs`), driven by the declared tiers; this file pins
//! what the registry does not enumerate: the nested SPA asset router, the
//! `config-ui` feature's two states, and the host wall over every route.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request, Response, StatusCode};
use tower::ServiceExt;

use crate::test_support::walled_fixture;
use crate::{AppState, build_router};

/// Sends one empty-bodied request through `build_router` with the
/// valid bearer key and the given peer address planted as the
/// `ConnectInfo` extension (or none at all).
async fn send_with_peer(
    state: AppState,
    method: Method,
    path: &str,
    peer: Option<&str>,
) -> Response<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(AUTHORIZATION, "Bearer test-token")
        .body(Body::empty())
        .expect("static request parts are valid");
    if let Some(peer) = peer {
        let peer: SocketAddr = peer.parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
    }
    build_router(state, None)
        .oneshot(request)
        .await
        .expect("the router is infallible")
}

#[tokio::test]
async fn admin_status_reports_a_stable_config_generation() {
    let (_temp, state) = walled_fixture();
    let first = send_with_peer(
        state.clone(),
        Method::GET,
        "/admin/status",
        Some("127.0.0.1:50000"),
    )
    .await;
    let second = send_with_peer(state, Method::GET, "/admin/status", Some("127.0.0.1:50000")).await;
    let first: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .expect("read first status"),
    )
    .expect("parse first status");
    let second: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(second.into_body(), usize::MAX)
            .await
            .expect("read second status"),
    )
    .expect("parse second status");
    let generation = first["config_generation"]
        .as_str()
        .expect("status generation is a string");
    assert!(
        !generation.is_empty(),
        "the generation identifies this process"
    );
    assert_eq!(
        generation,
        second["config_generation"]
            .as_str()
            .expect("second status generation is a string"),
        "one process reports one stable generation"
    );
}
#[cfg(feature = "config-ui")]
#[tokio::test]
async fn config_without_a_trailing_slash_redirects_to_the_mount() {
    let (_temp, state) = walled_fixture();
    let response = send_with_peer(state, Method::GET, "/config", Some("127.0.0.1:50000")).await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::LOCATION)
            .expect("a redirect carries a Location header"),
        "/config/",
        "the redirect lands on the trailing-slash mount so relative asset paths resolve"
    );
}

#[cfg(feature = "config-ui")]
#[tokio::test]
async fn the_config_ui_is_served_at_the_trailing_slash_mount() {
    let (_temp, state) = walled_fixture();
    for path in ["/config/", "/config/app.js"] {
        let status = send_with_peer(state.clone(), Method::GET, path, Some("127.0.0.1:50000"))
            .await
            .status();
        assert_eq!(status, StatusCode::OK, "GET {path} serves the SPA asset");
    }
}

#[cfg(feature = "config-ui")]
#[tokio::test]
async fn the_config_surface_refuses_a_lan_peer() {
    let (_temp, state) = walled_fixture();
    for path in ["/config", "/config/", "/config/app.js"] {
        let status = send_with_peer(state.clone(), Method::GET, path, Some("198.51.100.7:44821"))
            .await
            .status();
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "GET {path} must refuse a LAN peer"
        );
    }
}

#[cfg(not(feature = "config-ui"))]
#[tokio::test]
async fn without_the_feature_no_config_routes_exist() {
    let (_temp, state) = walled_fixture();
    for path in [
        "/config",
        "/config/",
        "/config/app.js",
        "/auth?key=test-token",
    ] {
        let status = send_with_peer(state.clone(), Method::GET, path, Some("127.0.0.1:50000"))
            .await
            .status();
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "GET {path} must not exist in a build without the config-ui feature"
        );
    }
}

/// Sends one request through `build_router` with the host wall
/// installed for `bound`, with the given `Host` header (or none).
async fn send_with_host(state: AppState, path: &str, host: Option<&str>) -> StatusCode {
    let bound: SocketAddr = "127.0.0.1:8081".parse().expect("a socket address");
    let mut builder = Request::builder()
        .uri(path)
        .header(AUTHORIZATION, "Bearer test-token");
    if let Some(host) = host {
        builder = builder.header(axum::http::header::HOST, host);
    }
    build_router(state, Some(bound))
        .oneshot(
            builder
                .body(Body::empty())
                .expect("static request parts are valid"),
        )
        .await
        .expect("the router is infallible")
        .status()
}

#[tokio::test]
async fn the_host_wall_refuses_a_foreign_host_on_every_route() {
    let (_temp, state) = walled_fixture();
    // `/health` is deliberately not exempt: the gateway-discovery-file probe
    // sends the bound address as Host, so the wall keeps it honest.
    for path in ["/health", "/admin/status", "/v1/models", "/shutdown"] {
        assert_eq!(
            send_with_host(state.clone(), path, Some("attacker.com")).await,
            StatusCode::FORBIDDEN,
            "{path} must refuse a rebound hostname"
        );
    }
}

#[tokio::test]
async fn the_host_wall_admits_the_bound_and_localhost_authorities() {
    let (_temp, state) = walled_fixture();
    for host in ["127.0.0.1:8081", "localhost:8081"] {
        assert_eq!(
            send_with_host(state.clone(), "/health", Some(host)).await,
            StatusCode::OK,
            "Host: {host} names the bound socket"
        );
    }
    // A missing authority fails closed.
    assert_eq!(
        send_with_host(state.clone(), "/health", None).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn the_router_seam_without_a_bound_address_carries_no_host_wall() {
    let (_temp, state) = walled_fixture();
    let response = build_router(state, None)
        .oneshot(
            Request::builder()
                .uri("/health")
                .header(axum::http::header::HOST, "attacker.com")
                .body(Body::empty())
                .expect("static request parts are valid"),
        )
        .await
        .expect("the router is infallible");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the `Gateway::router` seam has no bound socket to allowlist"
    );
}
