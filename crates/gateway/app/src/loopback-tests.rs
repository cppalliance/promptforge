//! The loopback and host-authority walls through the real router and a real listener.
//! The shared loopback wall over the admin config surface: every
//! walled path refuses a LAN peer with 403 even when it presents the
//! valid bearer key, admits a loopback peer past the wall, and fails
//! closed when no peer address exists; the bearer-only routes stay
//! reachable from any source. The `config-ui` feature's `/config`
//! mount and redirect are pinned here too, in both feature states.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request, Response, StatusCode};
use gateway_config::Config;
use tower::ServiceExt;

use crate::test_support::{AdminPaths, app_state};
use crate::{AppState, build_router};

/// A tempdir-backed state with real profiles and boot files, so every
/// walled handler has something to answer with once past the wall.
fn fixture() -> (tempfile::TempDir, AppState) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let models = temp.path().join("cache").join("models");
    std::fs::create_dir_all(&models).expect("mkdir cache models");
    let boot = temp.path().join("gateway.toml");
    std::fs::write(&boot, "").expect("write boot");
    let config = Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = '{cache}'
"#,
        cache = temp.path().join("cache").display(),
    ))
    .expect("the fixture profile parses");
    let state = app_state(
        config,
        Some(AdminPaths {
            fixture_dir: temp.path().to_path_buf(),
            active: "main".to_owned(),
            config_path: boot,
        }),
    );
    (temp, state)
}

/// Every admin config path behind the shared wall, with the method
/// exercised against it. The HF requests are deliberately malformed
/// (a duplicate query key, a slashless repo) so a loopback sweep is
/// refused at validation and never reaches the real hub; every other
/// empty-bodied write fails its own extractor the same way. All of
/// that happens past the wall, so any non-403 status proves
/// admission.
fn walled_requests() -> Vec<(Method, &'static str)> {
    let requests = vec![
        (Method::GET, "/admin/config"),
        (Method::PUT, "/admin/config"),
        (Method::GET, "/admin/env"),
        (Method::PUT, "/admin/env"),
        (Method::GET, "/admin/config-pending"),
        (Method::GET, "/admin/config-dirty"),
        (Method::POST, "/admin/config-apply"),
        (Method::POST, "/admin/config-revert"),
        (Method::GET, "/admin/system"),
        (Method::GET, "/admin/cloud-models"),
        (Method::POST, "/admin/cloud-models/refresh"),
        (Method::GET, "/admin/hf/search?q=a&q=b"),
        (Method::GET, "/admin/hf/model/owner/na%20me"),
        (Method::POST, "/admin/reveal"),
    ];
    #[cfg(feature = "local")]
    let requests = {
        let mut requests = requests;
        requests.extend([
            (Method::GET, "/admin/chat-templates"),
            (Method::GET, "/admin/orphans"),
            (Method::GET, "/admin/model-info"),
        ]);
        requests
    };
    requests
}

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
async fn every_walled_path_refuses_a_lan_peer_with_403() {
    let (_temp, state) = fixture();
    for (method, path) in walled_requests() {
        let status = send_with_peer(
            state.clone(),
            method.clone(),
            path,
            Some("198.51.100.7:44821"),
        )
        .await
        .status();
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} must refuse a LAN peer even with the valid bearer key"
        );
    }
}

#[tokio::test]
async fn every_walled_path_admits_a_loopback_peer_past_the_wall() {
    let (_temp, state) = fixture();
    for (method, path) in walled_requests() {
        let status = send_with_peer(state.clone(), method.clone(), path, Some("127.0.0.1:50000"))
            .await
            .status();
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} must pass the wall for a loopback peer"
        );
    }
}

#[tokio::test]
async fn every_walled_path_fails_closed_without_a_peer_address() {
    let (_temp, state) = fixture();
    for (method, path) in walled_requests() {
        let status = send_with_peer(state.clone(), method.clone(), path, None)
            .await
            .status();
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} must fail closed when the peer address is unknown"
        );
    }
}

#[tokio::test]
async fn the_bearer_only_routes_stay_reachable_from_the_lan() {
    let (_temp, state) = fixture();
    for path in [
        "/admin/status",
        "/admin/profiles",
        "/admin/progress",
        "/v1/models",
    ] {
        let status = send_with_peer(state.clone(), Method::GET, path, Some("198.51.100.7:44821"))
            .await
            .status();
        assert_eq!(
            status,
            StatusCode::OK,
            "GET {path} keeps its bearer-only, any-source behavior"
        );
    }
    // The switch route stays any-source too; the empty body fails its
    // own extractor past auth, so any non-403 status proves the wall
    // is absent (the same trick as the loopback-admission sweep).
    let status = send_with_peer(
        state.clone(),
        Method::POST,
        "/admin/switch-profile",
        Some("198.51.100.7:44821"),
    )
    .await
    .status();
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "POST /admin/switch-profile keeps its bearer-only, any-source behavior"
    );
    // The queue-cancel routes share the bearer-only, any-source
    // posture: cancelling a command mutates no configuration.
    for path in ["/admin/queue/cancel", "/admin/queue/cancel-pending"] {
        let status = send_with_peer(
            state.clone(),
            Method::POST,
            path,
            Some("198.51.100.7:44821"),
        )
        .await
        .status();
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "POST {path} keeps its bearer-only, any-source behavior"
        );
    }
}

#[tokio::test]
async fn admin_status_reports_a_stable_config_generation() {
    let (_temp, state) = fixture();
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
    let (_temp, state) = fixture();
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
    let (_temp, state) = fixture();
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
    let (_temp, state) = fixture();
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
    let (_temp, state) = fixture();
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
    let (_temp, state) = fixture();
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
    let (_temp, state) = fixture();
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
    let (_temp, state) = fixture();
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
