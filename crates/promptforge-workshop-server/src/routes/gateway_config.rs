//! The gateway-config panel's server side.
//!
//! Two routes serve the workshop UI's embedded config panel. `GET
//! /gateway/origin` reports the gateway's base URL so the UI can build
//! the panel iframe's address. `/gateway/api/{*path}` is the narrow
//! proxy the panel's postMessage bridge calls: it forwards allowlisted
//! config-surface requests to the gateway with the bearer key attached,
//! so the key never reaches any browser context. The routes mount
//! inside the API group, so the [`crate::cross_site`] guard applies and
//! a non-loopback `Host` is refused; the workshop listener itself binds
//! loopback only, which keeps the whole proxy unreachable from the LAN.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};

use crate::app::AppState;
use crate::deadline::{DEFAULT_DEADLINE, with_deadline};
use crate::error::AppError;

/// The gateway-config panel routes. The origin probe is local and
/// instant, so it carries the default deadline; the forward route is
/// added after the layer and carries none, because a forwarded cache
/// download or profile switch legitimately streams for minutes and the
/// gateway client already bounds the header phase.
pub(crate) fn routes(state: AppState) -> Router {
    with_deadline(
        Router::new().route("/gateway/origin", get(gateway_origin)),
        DEFAULT_DEADLINE,
    )
    .route("/gateway/api/{*path}", any(gateway_forward))
    .with_state(state)
}

/// Exact gateway paths the proxy forwards: the admin config surface the
/// config UI uses, plus the model catalog and the artifact cache.
const FORWARD_EXACT: &[&str] = &[
    "/admin/boot-config",
    "/admin/config",
    "/admin/config-apply",
    "/admin/config-dirty",
    "/admin/config-pending",
    "/admin/config-revert",
    "/admin/env",
    "/admin/model-info",
    "/admin/orphans",
    "/admin/profiles",
    "/admin/reveal",
    "/admin/status",
    "/admin/switch-profile",
    "/admin/system",
    "/v1/cache",
    "/v1/models",
];

/// Path prefixes the proxy forwards: per-name profile files,
/// include-file shadows, the HF proxy, and per-digest cache deletes.
const FORWARD_PREFIX: &[&str] = &[
    "/admin/hf/",
    "/admin/include/",
    "/admin/profiles/",
    "/v1/cache/",
];

/// Whether the proxy forwards `path`. Everything outside the allowlist
/// is refused: chat completions, `/admin/progress` (the workshop owns
/// progress display, so the panel must never subscribe), `/health`, and
/// the config UI assets. Dot segments are refused outright, because the
/// forwarding URL parse would normalize them and a `..` inside an
/// allowlisted prefix could otherwise escape onto a refused path;
/// backslashes are refused for the same reason (the WHATWG parse folds
/// them into slashes).
fn forward_allowed(path: &str) -> bool {
    if path
        .split('/')
        .any(|segment| segment == "." || segment == ".." || segment.contains('\\'))
    {
        return false;
    }
    FORWARD_EXACT.contains(&path) || FORWARD_PREFIX.iter().any(|prefix| path.starts_with(prefix))
}

/// Answers the gateway's base URL, so the workshop UI can point the
/// config panel's iframe at `<origin>/config/?mode=panel`.
async fn gateway_origin(State(state): State<AppState>) -> Response {
    let body = serde_json::json!({ "origin": state.gateway_client().base_url() });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Forwards one allowlisted request to the gateway with the bearer key
/// attached, relaying status, content type, and body byte-for-byte.
async fn gateway_forward(
    State(state): State<AppState>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    method: Method,
    body: Bytes,
) -> Result<Response, AppError> {
    let path = format!("/{path}");
    if !forward_allowed(&path) {
        return Err(AppError::ForwardDenied);
    }
    let path_and_query = match query {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    // The axum and reqwest method types come from potentially different
    // `http` crate versions, so the conversion goes through the name; a
    // name reqwest cannot represent is refused rather than forwarded.
    let method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .map_err(|_| AppError::ForwardDenied)?;
    let forwarded = state
        .gateway_client()
        .forward(
            method,
            &path_and_query,
            (!body.is_empty()).then(|| body.to_vec()),
        )
        .await
        .map_err(AppError::Gateway)?;
    let status = StatusCode::from_u16(forwarded.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = &forwarded.content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    // The parts are valid by construction (the status came off the wire,
    // the content type round-tripped a valid header), so the build
    // cannot fail; a failure would be a bug, answered as a plain 502.
    Ok(builder
        .body(Body::from(forwarded.body))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::Request;
    use axum::response::IntoResponse;
    use axum::routing::{get as axum_get, post as axum_post};
    use tower::ServiceExt;

    use crate::app::fixtures::{body_bytes, spawn_gateway, state_for};
    use crate::app::router;

    #[test]
    fn the_allowlist_admits_the_config_surface_and_refuses_the_rest() {
        for path in [
            "/admin/config",
            "/admin/config-apply",
            "/admin/status",
            "/admin/profiles",
            "/admin/profiles/beta",
            "/admin/include/common.toml",
            "/admin/hf/search",
            "/admin/switch-profile",
            "/v1/models",
            "/v1/cache",
            "/v1/cache/abc123",
        ] {
            assert!(forward_allowed(path), "{path} must be forwardable");
        }
        for path in [
            "/v1/chat/completions",
            "/admin/progress",
            "/health",
            "/config/",
            "/admin/hf/../../v1/chat/completions",
            "/admin/hf/..\\..\\v1\\chat\\completions",
            "/admin/hf/./search",
            "/admin",
        ] {
            assert!(!forward_allowed(path), "{path} must be refused");
        }
    }

    #[tokio::test]
    async fn the_origin_route_answers_the_configured_gateway_base_url() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:8081");
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
        let (state, _tape_dir) = state_for(&base_url);
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
    async fn the_proxy_forwards_the_query_string_and_a_json_body() {
        let gateway = axum::Router::new().route(
            "/admin/switch-profile",
            axum_post(
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
        let (state, _tape_dir) = state_for(&base_url);
        let request = Request::builder()
            .method("POST")
            .uri("/gateway/api/admin/switch-profile")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"beta"}"#))
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&body_bytes(response).await).expect("the body is JSON");
        assert_eq!(json["declared_json"], true, "the body forwards as JSON");
        assert_eq!(json["echo"]["name"], "beta", "the body forwards verbatim");
    }

    #[tokio::test]
    async fn the_proxy_refuses_a_non_allowlisted_path_without_dialing() {
        // An unroutable gateway address: a refused path must answer 403
        // before any dial, so no transport error can occur.
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
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
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
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
}
