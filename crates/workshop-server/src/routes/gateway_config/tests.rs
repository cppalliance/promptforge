use super::*;

use axum::body::Body;
use axum::http::Request;
use axum::response::IntoResponse;
use axum::routing::{get as axum_get, put as axum_put};
use tower::ServiceExt;

use crate::app::fixtures::{body_bytes, spawn_gateway, state_for};
use crate::app::router;

#[test]
fn the_allowlist_admits_the_config_surface_and_refuses_the_rest() {
    for (method, path) in [
        (Method::GET, "/admin/config"),
        (Method::GET, "/admin/chat-templates"),
        (Method::PUT, "/admin/config"),
        (Method::POST, "/admin/config-apply"),
        (Method::POST, "/admin/config-revert"),
        (Method::POST, "/admin/queue/cancel"),
        (Method::POST, "/admin/queue/cancel-pending"),
        (Method::GET, "/admin/status"),
        (Method::GET, "/admin/hf/search"),
        (Method::GET, "/v1/cache"),
        (
            Method::DELETE,
            "/v1/cache/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
    ] {
        assert!(
            forward_allowed(&method, path),
            "{method} {path} must be forwardable"
        );
    }
    for (method, path) in [
        (Method::POST, "/v1/cache"),
        (Method::GET, "/v1/models"),
        (Method::POST, "/v1/chat/completions"),
        (Method::GET, "/admin/progress"),
        (Method::PUT, "/admin/boot-config"),
        (Method::PUT, "/admin/include/common.toml"),
        (Method::POST, "/admin/profiles/beta"),
        (Method::POST, "/admin/switch-profile"),
        (Method::GET, "/health"),
        (Method::GET, "/config/"),
        (Method::GET, "/admin/hf/../../v1/chat/completions"),
        (Method::GET, "/admin/hf/..\\..\\v1\\chat\\completions"),
        (Method::GET, "/admin/hf/./search"),
        (Method::GET, "/admin"),
        (Method::DELETE, "/v1/cache/abc123"),
    ] {
        assert!(
            !forward_allowed(&method, path),
            "{method} {path} must be refused"
        );
    }
}

#[tokio::test]
async fn the_origin_route_answers_the_configured_gateway_base_url() {
    let (state, _state_dir) = state_for("http://127.0.0.1:8081");
    let request = Request::builder()
        .uri("/gateway/origin")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON");
    assert_eq!(json["origin"], "http://127.0.0.1:8081");
}

#[tokio::test]
async fn the_proxy_forwards_an_allowlisted_path_with_the_bearer_key() {
    let gateway = axum::Router::new().route(
        "/admin/status",
        axum_get(|headers: axum::http::HeaderMap| async move {
            let authorized = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                == Some("Bearer test-key");
            (
                [(header::CONTENT_TYPE, "application/json")],
                format!(r#"{{"profile":"default","authorized":{authorized}}}"#),
            )
        }),
    );
    let base_url = spawn_gateway(gateway).await;
    let (state, _state_dir) = state_for(&base_url);
    let request = Request::builder()
        .uri("/gateway/api/admin/status")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json"),
        "the gateway's content type is relayed"
    );
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON");
    assert_eq!(json["profile"], "default", "the body is relayed verbatim");
    assert_eq!(
        json["authorized"], true,
        "the forward carries the workshop's bearer key"
    );
}

#[tokio::test]
async fn the_proxied_config_assets_force_revalidation() {
    let gateway = axum::Router::new().route(
        "/config/app.js",
        axum_get(|| async move {
            (
                [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                "// bundle",
            )
        }),
    );
    let base_url = spawn_gateway(gateway).await;
    let (state, _state_dir) = state_for(&base_url);
    let request = Request::builder()
        .uri("/gateway/config/app.js")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    // The relayed bundle is unversioned; without this the panel's
    // WebView2 serves a cached script against a newer gateway.
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache"),
        "the relay forces revalidation"
    );
}

#[tokio::test]
async fn the_proxy_forwards_the_query_string_and_a_json_body() {
    let gateway = axum::Router::new().route(
        "/admin/config",
        axum_put(
            |headers: axum::http::HeaderMap, request: axum::extract::Request| async move {
                let declared_json = headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    == Some("application/json");
                let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                    .await
                    .unwrap_or_default();
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    format!(
                        r#"{{"declared_json":{declared_json},"echo":{}}}"#,
                        String::from_utf8_lossy(&body)
                    ),
                )
                    .into_response()
            },
        ),
    );
    let base_url = spawn_gateway(gateway).await;
    let (state, _state_dir) = state_for(&base_url);
    let request = Request::builder()
        .method("PUT")
        .uri("/gateway/api/admin/config?source=panel")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"active_profile":"beta"}"#))
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON");
    assert_eq!(json["declared_json"], true, "the body forwards as JSON");
    assert_eq!(
        json["echo"]["active_profile"], "beta",
        "the body forwards verbatim"
    );
}

#[tokio::test]
async fn the_proxy_refuses_a_non_allowlisted_path_without_dialing() {
    // An unroutable gateway address: a refused path must answer 403
    // before any dial, so no transport error can occur.
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    for path in [
        "/gateway/api/v1/chat/completions",
        "/gateway/api/admin/progress",
        "/gateway/api/admin/hf/../../v1/chat/completions",
    ] {
        let request = Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state.clone())
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "for {path}");
        let json: serde_json::Value =
            serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON");
        assert_eq!(json["error"]["code"], "forward_denied", "for {path}");
    }
}

#[tokio::test]
async fn the_proxy_sits_behind_the_cross_site_guard() {
    // The workshop listener binds loopback only; on top of that the
    // cross-site guard refuses a DNS-rebound Host, so the proxy is
    // covered by the same wall as the rest of the API surface.
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .uri("/gateway/api/admin/status")
        .header("host", "rebound.example:7910")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON");
    assert_eq!(json["error"]["code"], "cross_site");
}
