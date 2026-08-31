//! Cross-site request blocking for the loopback workshop server.
//!
//! Any webpage the user visits can issue requests to `127.0.0.1`, so
//! without these checks the workspace-write and WebSocket endpoints are
//! open to CSRF and DNS rebinding. Three layers close the hole. The
//! [`guard`] middleware wraps the API router: it refuses any request a
//! browser marks `Sec-Fetch-Site: cross-site`, refuses any request whose
//! `Host` names a non-loopback authority (a DNS-rebound page's requests
//! are same-origin under Sec-Fetch and can declare any content type, so
//! the attacker's hostname in `Host` is the one signal rebinding cannot
//! forge), and requires body-bearing requests to declare
//! `application/json`, which forces a CORS preflight no cross-site page
//! passes. WebSocket upgrades bypass Sec-Fetch in older browsers, so both
//! upgrade handlers additionally check
//! [`origin_allowed`]: an `Origin` header, when present, must be a
//! loopback http(s) origin - which admits both the shell webview (it loads
//! the workshop's own loopback URL) and a browser tab on the workshop's
//! address, and refuses every foreign site. A request with no `Origin` is
//! a native client, not a browser, and passes. `/health` and the UI
//! assets stay outside the guard so the shell probe and heartbeat keep
//! working.

use axum::extract::Request;
use axum::http::{HeaderMap, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// Refuses cross-site requests, rebound hosts, and non-JSON bodies on the
/// API router.
///
/// A request carrying `Sec-Fetch-Site: cross-site` or naming a
/// non-loopback `Host` answers 403; a POST, PUT, or PATCH whose
/// `Content-Type` is not `application/json` answers 415. Everything else
/// passes through.
pub async fn guard(request: Request, next: Next) -> Response {
    if request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|site| site.as_bytes().eq_ignore_ascii_case(b"cross-site"))
    {
        return AppError::CrossSite.into_response();
    }
    if !host_is_loopback(&request) {
        return AppError::CrossSite.into_response();
    }
    let has_body = [Method::POST, Method::PUT, Method::PATCH].contains(request.method());
    if has_body && !declares_json(request.headers()) {
        return AppError::NotJson.into_response();
    }
    next.run(request).await
}

/// Whether the request's authority is loopback, closing DNS rebinding: a
/// page on a rebound hostname reaches this server with same-origin
/// Sec-Fetch metadata and a freely chosen content type, but its `Host`
/// still carries the attacker's name. The URI authority (HTTP/2,
/// absolute-form) wins over the `Host` header. A request naming no
/// authority at all passes - browsers always send `Host`, so that is a
/// native client.
fn host_is_loopback(request: &Request) -> bool {
    let authority = match request.uri().authority() {
        Some(authority) => authority.as_str(),
        None => match request.headers().get(header::HOST) {
            Some(host) => match host.to_str() {
                Ok(host) => host,
                Err(_) => return false,
            },
            None => return true,
        },
    };
    is_loopback_origin(&format!("http://{authority}"))
}

/// Whether the request declares an `application/json` body, ignoring
/// parameters such as `charset`.
fn declares_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or_default().trim())
        .is_some_and(|essence| essence.eq_ignore_ascii_case("application/json"))
}

/// Whether a WebSocket upgrade's `Origin` is acceptable: absent (a native
/// client), or a loopback http(s) origin - the shell webview and the
/// workshop's own browser-tab origin are both loopback.
pub fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    origin.to_str().is_ok_and(is_loopback_origin)
}

/// Whether `origin` parses as an http(s) URL whose host is loopback. The
/// opaque `null` origin (sandboxed iframes) does not parse and is refused.
fn is_loopback_origin(origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tokio_tungstenite::tungstenite;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tower::ServiceExt;

    use crate::app::fixtures::{body_bytes, config_for, state_for};
    use crate::app::router;

    #[test]
    fn loopback_origins_are_allowed() {
        for origin in [
            "http://127.0.0.1:7910",
            "http://127.5.0.1",
            "http://localhost:7910",
            "http://LOCALHOST",
            "http://[::1]:7910",
            "https://127.0.0.1",
        ] {
            assert!(is_loopback_origin(origin), "{origin} must be allowed");
        }
    }

    #[test]
    fn foreign_and_malformed_origins_are_refused() {
        for origin in [
            "https://evil.example",
            "http://192.168.1.10:7910",
            "https://localhost.evil.example",
            "file:///etc/passwd",
            "null",
            "",
        ] {
            assert!(!is_loopback_origin(origin), "{origin} must be refused");
        }
    }

    #[test]
    fn an_absent_origin_is_a_native_client_and_passes() {
        assert!(origin_allowed(&HeaderMap::new()));
    }

    /// Reads the `error.code` of a JSON error envelope.
    async fn envelope_code(response: axum::response::Response) -> String {
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("the body is JSON");
        json["error"]["code"]
            .as_str()
            .expect("the envelope carries a code")
            .to_owned()
    }

    #[tokio::test]
    async fn cross_site_requests_are_refused_and_health_stays_exempt() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let app = router(state);
        for path in ["/v1/models", "/ws", "/workspace/tree"] {
            let request = Request::builder()
                .uri(path)
                .header("sec-fetch-site", "cross-site")
                .body(Body::empty())
                .expect("static request parts are valid");
            let response = app
                .clone()
                .oneshot(request)
                .await
                .expect("the router is infallible");
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "for {path}");
            assert_eq!(envelope_code(response).await, "cross_site", "for {path}");
        }
        for site in ["same-origin", "none"] {
            let request = Request::builder()
                .uri("/workspace/tree")
                .header("sec-fetch-site", site)
                .body(Body::empty())
                .expect("static request parts are valid");
            let response = app
                .clone()
                .oneshot(request)
                .await
                .expect("the router is infallible");
            assert_eq!(response.status(), StatusCode::OK, "for {site}");
        }
        let request = Request::builder()
            .uri("/health")
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = app
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "/health stays exempt for the shell probe and heartbeat"
        );
    }

    #[tokio::test]
    async fn a_dns_rebound_host_is_refused() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let app = router(state);
        // A rebound page's fetch arrives same-origin under Sec-Fetch with
        // any content type it likes; only Host betrays it.
        let request = Request::builder()
            .uri("/workspace/tree")
            .header("host", "rebound.example:7910")
            .header("sec-fetch-site", "same-origin")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(envelope_code(response).await, "cross_site");
        for host in ["127.0.0.1:7910", "localhost:7910", "[::1]:7910"] {
            let request = Request::builder()
                .uri("/workspace/tree")
                .header("host", host)
                .body(Body::empty())
                .expect("static request parts are valid");
            let response = app
                .clone()
                .oneshot(request)
                .await
                .expect("the router is infallible");
            assert_eq!(response.status(), StatusCode::OK, "for {host}");
        }
    }

    #[tokio::test]
    async fn post_bodies_must_declare_json() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let app = router(state);
        let dir = tempfile::TempDir::new().expect("tempdir");
        let body = serde_json::json!({ "path": dir.path() }).to_string();

        let refused = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/workspace/grant")
                    .header("content-type", "text/plain")
                    .body(Body::from(body.clone()))
                    .expect("static request parts are valid"),
            )
            .await
            .expect("the router is infallible");
        assert_eq!(refused.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(envelope_code(refused).await, "not_json");

        let accepted = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/workspace/grant")
                    .header("content-type", "application/json; charset=utf-8")
                    .body(Body::from(body))
                    .expect("static request parts are valid"),
            )
            .await
            .expect("the router is infallible");
        assert_eq!(
            accepted.status(),
            StatusCode::OK,
            "a declared JSON body passes the guard, parameters and all"
        );
    }

    /// Connects a WebSocket to `path` on the live server with an optional
    /// `Origin` header, returning the handshake outcome.
    async fn ws_connect(
        base: &str,
        path: &str,
        origin: Option<&str>,
    ) -> Result<(), tungstenite::Error> {
        let address = base.strip_prefix("http://").expect("the URL is http");
        let mut request = format!("ws://{address}{path}")
            .into_client_request()
            .expect("the handshake request builds");
        if let Some(origin) = origin {
            request.headers_mut().insert(
                "origin",
                origin.parse().expect("the origin is a valid header value"),
            );
        }
        tokio_tungstenite::connect_async(request).await.map(|_| ())
    }

    #[tokio::test]
    async fn ws_upgrades_enforce_the_origin_allowlist() {
        let tape_dir = tempfile::TempDir::new().expect("tempdir");
        let mut config = config_for("http://127.0.0.1:1", &tape_dir.path().join("tape.jsonl"));
        config.server.bind = "127.0.0.1:0".to_string();
        let server = crate::serve::spawn(config).expect("server spawns");
        let url = server.url().to_string();
        for path in ["/ws"] {
            ws_connect(&url, path, None)
                .await
                .expect("a native client with no Origin upgrades");
            ws_connect(&url, path, Some(&url))
                .await
                .expect("the workshop's own loopback origin (the shell webview) upgrades");
            let error = ws_connect(&url, path, Some("https://evil.example"))
                .await
                .expect_err("a cross-site origin must be refused");
            match error {
                tungstenite::Error::Http(response) => assert_eq!(
                    response.status(),
                    tungstenite::http::StatusCode::FORBIDDEN,
                    "for {path}"
                ),
                other => panic!("expected an HTTP 403 refusal for {path}, got {other:?}"),
            }
        }
        server.shutdown().expect("graceful shutdown succeeds");
    }
}
