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
    // The config SPA assets, proxied same-origin so the panel iframe
    // loads from the workshop's own origin instead of the gateway's
    // port. A cross-origin iframe makes Chromium spawn renderer
    // processes that flash a console window on some Windows configs.
    .route("/gateway/config/", get(gateway_config_index))
    .route("/gateway/config/{*path}", get(gateway_config_assets))
    .with_state(state)
}

/// Whether the proxy forwards `method` and `path`. Everything outside the allowlist
/// is refused: chat completions, `/admin/progress` (the workshop owns
/// progress display, so the panel must never subscribe), `/health`, and
/// the config UI assets. Dot segments are refused outright, because the
/// forwarding URL parse would normalize them and a `..` inside an
/// allowlisted prefix could otherwise escape onto a refused path;
/// backslashes are refused for the same reason (the WHATWG parse folds
/// them into slashes). Methods are part of the allowlist, so the removed
/// direct-download flow cannot reach `POST /v1/cache` through the panel.
fn forward_allowed(method: &Method, path: &str) -> bool {
    if path
        .split('/')
        .any(|segment| segment == "." || segment == ".." || segment.contains('\\'))
    {
        return false;
    }
    match *method {
        Method::GET => {
            matches!(
                path,
                "/admin/config"
                    | "/admin/chat-templates"
                    | "/admin/config-dirty"
                    | "/admin/config-pending"
                    | "/admin/env"
                    | "/admin/model-info"
                    | "/admin/orphans"
                    | "/admin/status"
                    | "/admin/system"
                    | "/v1/cache"
            ) || path.starts_with("/admin/hf/")
        }
        Method::PUT => matches!(path, "/admin/config" | "/admin/env"),
        Method::POST => matches!(
            path,
            "/admin/config-apply"
                | "/admin/config-revert"
                | "/admin/reveal"
                | "/admin/queue/cancel"
                | "/admin/queue/cancel-pending"
        ),
        Method::DELETE => path.strip_prefix("/v1/cache/").is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        }),
        _ => false,
    }
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

/// Proxies the config SPA's index page from the gateway so the panel
/// iframe loads same-origin.
async fn gateway_config_index(State(state): State<AppState>) -> Result<Response, AppError> {
    proxy_config_asset(&state, "/config/").await
}

/// Proxies the config SPA's sub-assets (JS, CSS, icons) from the gateway.
async fn gateway_config_assets(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Response, AppError> {
    let path = format!("/config/{path}");
    if path
        .split('/')
        .any(|segment| segment == "." || segment == ".." || segment.contains('\\'))
    {
        return Err(AppError::ForwardDenied);
    }
    proxy_config_asset(&state, &path).await
}

/// The shared proxy core: GET the gateway's config asset and relay it.
async fn proxy_config_asset(state: &AppState, path: &str) -> Result<Response, AppError> {
    let forwarded = state
        .gateway_client()
        .forward(reqwest::Method::GET, path, None)
        .await
        .map_err(AppError::Gateway)?;
    let status = StatusCode::from_u16(forwarded.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = &forwarded.content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    // The relayed SPA assets are unversioned; force revalidation so the
    // panel never runs a cached script against a newer gateway.
    builder = builder.header(header::CACHE_CONTROL, "no-cache");
    Ok(builder
        .body(Body::from(forwarded.body))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()))
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
    if !forward_allowed(&method, &path) {
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
mod tests;
