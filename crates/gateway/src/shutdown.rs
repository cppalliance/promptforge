//! The `POST /shutdown` route and the process-shutdown signal it fires.
//!
//! The route is the remote face of the graceful shutdown that Ctrl-C and
//! [`GatewayHandle::shutdown`](crate::GatewayHandle::shutdown) drive; the
//! tray's Quit and the shell's Quit-everything call it. It sits behind the
//! shared loopback wall and bearer auth, and it answers `202 Accepted`
//! while its own request is still in flight: axum's graceful shutdown
//! drains in-flight requests before closing their connections, so the
//! response always reaches the caller ahead of the shutdown it asked for.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};

use crate::AppState;
use crate::error::GatewayError;

/// The process-shutdown signal shared by the `POST /shutdown` route and
/// the serve loop, which selects on it alongside the caller-owned shutdown
/// future.
///
/// Every clone shares the one underlying signal. A `fire` that lands
/// before the serve loop waits is not lost: the notify stores a single
/// permit, so the pending `fired` resolves immediately.
#[derive(Debug, Clone, Default)]
pub(crate) struct ShutdownSignal {
    notify: Arc<tokio::sync::Notify>,
    /// Set by `fire` before the notify, so a synchronous reader (the
    /// tray's status tick) can tell a requested shutdown apart from a
    /// serve-loop failure without consuming the permit.
    fired: Arc<AtomicBool>,
}

impl ShutdownSignal {
    /// Fires the signal, starting the serve loop's graceful shutdown.
    pub(crate) fn fire(&self) {
        self.fired.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    /// Whether the signal has been fired.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    pub(crate) fn is_fired(&self) -> bool {
        self.fired.load(Ordering::Acquire)
    }

    /// Resolves once the signal has fired.
    pub(crate) async fn fired(&self) {
        self.notify.notified().await;
    }
}

/// The `POST /shutdown` route: bearer-authed, loopback-only via the shared
/// wall, answering `202 Accepted` and firing the shutdown signal.
///
/// Like every bearer route it inherits the configured key, including the
/// deliberately credential-free empty-key configuration.
pub(crate) async fn admin_shutdown(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, GatewayError> {
    crate::check_auth(&state, &headers).await?;
    // Cancel the active queue command first: a shutdown during provisioning
    // stops the download, so the serve loop's drain and the process exit
    // stay prompt.
    state.commands.cancel_active();
    state.shutdown.fire();
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Duration;

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{Method, Request, Response, StatusCode};
    use gateway_config::Config;
    use tower::ServiceExt;

    use crate::test_support::app_state;
    use crate::{AppState, build_router};

    fn state() -> AppState {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
        )
        .expect("config parses");
        app_state(config, None)
    }

    /// Sends one request to `/shutdown` through the router with the given
    /// bearer key and peer address planted, as the walled route requires.
    async fn send(
        state: &AppState,
        method: Method,
        key: Option<&str>,
        peer: &str,
    ) -> Response<Body> {
        let mut builder = Request::builder().method(method).uri("/shutdown");
        if let Some(key) = key {
            builder = builder.header(AUTHORIZATION, format!("Bearer {key}"));
        }
        let mut request = builder.body(Body::empty()).expect("request builds");
        let peer: SocketAddr = peer.parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
        build_router(state.clone(), None)
            .oneshot(request)
            .await
            .expect("the router is infallible")
    }

    /// The tray's status tick reads `is_fired` synchronously to tell a
    /// requested shutdown apart from a serve-loop failure; the method is
    /// gated on the tray backends like its only callers.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    #[test]
    fn fire_sets_the_synchronous_peek() {
        let signal = super::ShutdownSignal::default();
        assert!(!signal.is_fired(), "a fresh signal reads unfired");
        signal.fire();
        assert!(signal.is_fired(), "fire sets the peek before any wait");
    }

    #[tokio::test]
    async fn the_route_answers_202_and_fires_the_signal() {
        let state = state();
        let response = send(&state, Method::POST, Some("test-token"), "127.0.0.1:50000").await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        tokio::time::timeout(Duration::from_secs(5), state.shutdown.fired())
            .await
            .expect("the route fired the shutdown signal");
    }

    #[tokio::test]
    async fn the_route_rejects_a_missing_or_wrong_key_without_firing() {
        let state = state();
        for key in [None, Some("wrong")] {
            let response = send(&state, Method::POST, key, "127.0.0.1:50000").await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "key {key:?}");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), state.shutdown.fired())
                .await
                .is_err(),
            "a refused request must leave the server up"
        );
    }

    #[tokio::test]
    async fn the_route_rejects_non_post_methods() {
        let state = state();
        for method in [Method::GET, Method::PUT, Method::DELETE] {
            let response = send(
                &state,
                method.clone(),
                Some("test-token"),
                "127.0.0.1:50000",
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} must not reach the handler"
            );
        }
    }

    #[tokio::test]
    async fn the_route_refuses_a_lan_peer_even_with_the_key() {
        let state = state();
        let response = send(
            &state,
            Method::POST,
            Some("test-token"),
            "198.51.100.7:44821",
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), state.shutdown.fired())
                .await
                .is_err(),
            "a walled-off request must leave the server up"
        );
    }
}
