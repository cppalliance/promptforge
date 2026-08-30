//! The shared loopback wall for the PromptForge gateway's config surface.
//!
//! One middleware, [`require_loopback`], refuses any request whose peer
//! address is not loopback. The config-ui crate wraps its SPA asset
//! routes with it (re-exporting it as its own public surface), and the
//! gateway applies the same function to its admin config endpoints, so
//! the check exists in exactly one place. The crate is deliberately tiny -
//! axum is its only dependency - because the gateway needs the wall in
//! every build, including headless builds that never compile the
//! config-ui crate and its embedded-asset machinery.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Refuses any request whose peer address is not loopback with
/// `403 Forbidden`, applied through [`axum::middleware::from_fn`].
///
/// This is the single shared loopback check for the whole config
/// surface: the config-ui crate's asset router wraps the SPA routes with
/// it, and the gateway applies the same function to its admin config
/// endpoints (the config read and write paths, env, orphans, system,
/// model-info, the HF proxy, profile create and delete, and reveal), so
/// the check exists in exactly one place. Those endpoints hold secrets in
/// plaintext and write files, so they must never be reachable from the
/// LAN even with the bearer key; the wall comes before auth.
///
/// The peer address is read from the [`ConnectInfo`] request extension,
/// which exists only when the server is started with
/// `into_make_service_with_connect_info::<SocketAddr>()`. A request with
/// no peer address fails closed: it is refused as non-loopback rather
/// than admitted on a wiring fault.
pub async fn require_loopback(request: Request, next: Next) -> Response {
    match request.extensions().get::<ConnectInfo<SocketAddr>>() {
        Some(ConnectInfo(peer)) if peer.ip().is_loopback() => next.run(request).await,
        _ => StatusCode::FORBIDDEN.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use tower::ServiceExt;

    use super::*;

    /// A one-route router with the loopback wall applied, mirroring how
    /// the config-ui asset router and the gateway layer it.
    fn guarded_router() -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(require_loopback))
    }

    /// Sends one request through the guarded router, with the given peer
    /// address planted as the `ConnectInfo` extension (or none at all).
    async fn status_for(peer: Option<&str>) -> StatusCode {
        let mut request = HttpRequest::builder()
            .uri("/")
            .body(Body::empty())
            .expect("static request parts are valid");
        if let Some(address) = peer {
            let address: SocketAddr = address.parse().expect("a socket address");
            request.extensions_mut().insert(ConnectInfo(address));
        }
        guarded_router()
            .oneshot(request)
            .await
            .expect("the router is infallible")
            .status()
    }

    #[tokio::test]
    async fn a_loopback_ipv4_peer_is_admitted() {
        assert_eq!(status_for(Some("127.0.0.1:50000")).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_loopback_ipv6_peer_is_admitted() {
        assert_eq!(status_for(Some("[::1]:50000")).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_lan_peer_is_refused_with_403() {
        assert_eq!(
            status_for(Some("198.51.100.7:44821")).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn a_missing_peer_address_fails_closed_with_403() {
        assert_eq!(status_for(None).await, StatusCode::FORBIDDEN);
    }
}
