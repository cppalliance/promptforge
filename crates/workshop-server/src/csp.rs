//! The Content-Security-Policy stamped on every response from the
//! server's own routes (routes a host merges through `spawn_with_routes`
//! are composed after this layer and carry their own layers).
//!
//! The desktop shell loads the UI as an External-origin Tauri webview, so
//! the page's policy is the server's to set: there is no `tauri.conf.json`
//! CSP for a remote document. The policy keeps the SPA self-contained -
//! scripts and workers from this origin only - while `connect-src` admits
//! the two IPC endpoints Tauri calls from an External origin (`ipc:` on
//! the custom-protocol platforms, `http://ipc.localhost` under WebView2)
//! and the loopback WebSocket spellings. WebKit does not treat
//! `connect-src 'self'` as covering WebSockets, so the `ws://` sources
//! are spelled out for WebKitGTK and WKWebView; the port wildcard covers
//! the shell's OS-assigned bind.

use axum::extract::Request;
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;

/// The policy value, one header for every response.
///
/// `img-src` keeps `https:` so model-written markdown can still render
/// remote images. `style-src` keeps `'unsafe-inline'` because the UI's
/// styling is partly runtime-injected and cannot be nonced: CodeMirror 6
/// mounts its modules as `<style>` elements through style-mod, and Shiki
/// emits inline `style` color attributes, which Chromium gates through
/// the `style-src-attr` fallback to `style-src`. The directives that gate
/// code execution and connections stay strict.
const POLICY: &str = "default-src 'self'; script-src 'self'; \
                      style-src 'self' 'unsafe-inline'; \
                      connect-src 'self' ipc: http://ipc.localhost ws://127.0.0.1:* \
                      ws://localhost:* ws://[::1]:*; img-src 'self' data: blob: https:; \
                      worker-src 'self'; object-src 'none'; base-uri 'none'; \
                      frame-ancestors 'none'";

/// The same policy with self-framing allowed, for the proxied config SPA
/// only: the Gateway Config panel iframes `/gateway/config/` from the
/// workshop window, and `frame-ancestors 'none'` makes Chromium refuse
/// the frame outright ("refused to connect"). `'self'` admits the
/// same-origin shell and still forbids every foreign framer.
const POLICY_FRAMEABLE: &str = "default-src 'self'; script-src 'self'; \
                      style-src 'self' 'unsafe-inline'; \
                      connect-src 'self' ipc: http://ipc.localhost ws://127.0.0.1:* \
                      ws://localhost:* ws://[::1]:*; img-src 'self' data: blob: https:; \
                      worker-src 'self'; object-src 'none'; base-uri 'none'; \
                      frame-ancestors 'self'";

/// Middleware stamping `Content-Security-Policy` on every response,
/// including error envelopes. The proxied config SPA (`/gateway/config/`)
/// carries the self-frameable variant; everything else forbids framing.
pub(crate) async fn header(request: Request, next: Next) -> Response {
    let frameable = request.uri().path().starts_with("/gateway/config/");
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(if frameable { POLICY_FRAMEABLE } else { POLICY }),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::app::fixtures::state_for;
    use crate::app::router;

    /// Fetches `uri` against the composed router and returns the response.
    async fn get(uri: &str) -> axum::response::Response {
        let (state, _state_dir) = state_for("http://127.0.0.1:1");
        router(state)
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("static request parts are valid"),
            )
            .await
            .expect("the router is infallible")
    }

    #[tokio::test]
    async fn every_response_carries_the_policy() {
        // The UI document, its asset, the health probe, and an API error
        // envelope: the header is stamped at the composition root, so no
        // response class escapes it.
        for uri in ["/", "/app.js", "/health", "/workspace/tree"] {
            let response = get(uri).await;
            let value = response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .unwrap_or_else(|| panic!("{uri} carries the policy"));
            assert_eq!(value, POLICY, "{uri} carries the exact policy");
        }
    }

    #[tokio::test]
    async fn the_proxied_config_spa_allows_self_framing() {
        // The Gateway Config panel iframes `/gateway/config/` from the
        // workshop window; `frame-ancestors 'none'` there makes Chromium
        // refuse the frame ("refused to connect"). The gateway is
        // unreachable in this fixture, so the answer is a 502 - the
        // middleware stamps the header on error envelopes too, which is
        // the point: the policy must not depend on the proxy's outcome.
        let response = get("/gateway/config/").await;
        let value = response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("the proxied config route carries the policy");
        assert_eq!(value, POLICY_FRAMEABLE, "self-framing for the panel");
    }

    #[tokio::test]
    async fn the_policy_admits_tauri_ipc_and_the_loopback_socket() {
        let response = get("/health").await;
        assert_eq!(response.status(), StatusCode::OK);
        let policy = response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("the policy header is present")
            .to_str()
            .expect("the policy is ASCII");
        // The break this pins: drop the IPC sources and the shell webview's
        // Tauri calls fail closed from the External origin.
        assert!(
            policy.contains("connect-src 'self' ipc: http://ipc.localhost"),
            "connect-src admits the Tauri IPC endpoints: {policy}"
        );
        assert!(
            policy.contains("ws://127.0.0.1:*"),
            "connect-src admits the loopback WebSocket: {policy}"
        );
    }

    #[tokio::test]
    async fn the_policy_admits_runtime_injected_styles() {
        let response = get("/health").await;
        let policy = response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("the policy header is present")
            .to_str()
            .expect("the policy is ASCII");
        // The break this pins: a self-only style policy refuses the
        // `<style>` elements CodeMirror 6 mounts through style-mod and
        // the inline `style` color attributes Shiki emits (Chromium gates
        // attributes through the style-src-attr fallback), leaving the
        // editor unstyled and code blocks uncolored.
        assert!(
            policy.contains("style-src 'self' 'unsafe-inline'"),
            "style-src admits the UI's runtime-injected styles: {policy}"
        );
    }
}
