//! The [`Caller`] extractor: what an authenticated handler knows about who
//! is asking - the request headers and, when the server recorded one, the
//! peer address. [`crate::check_auth`] reads both, so a handler that once
//! extracted a bare `HeaderMap` for auth now extracts a `Caller` and
//! changes nothing else: the extractor derefs to the header map.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::ops::Deref;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;

/// The request headers plus the peer address, as [`crate::check_auth`]
/// needs them.
///
/// The peer comes from the `ConnectInfo<SocketAddr>` extension, which
/// exists only when the server was started with
/// `into_make_service_with_connect_info::<SocketAddr>()`. The extractor
/// never rejects: a request with no peer address yields `peer: None`,
/// and the auth rule then fails closed by requiring a credential, the
/// same posture as the shared loopback wall. A wiring fault must cost the
/// caller a `401`, never a `500`.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    headers: HeaderMap,
    peer: Option<SocketAddr>,
}

impl Caller {
    /// The peer address the server recorded for this connection, when it
    /// recorded one.
    pub(crate) fn peer(&self) -> Option<SocketAddr> {
        self.peer
    }

    /// Assembles a caller from its parts, for tests that drive
    /// [`crate::check_auth`] directly rather than through the router.
    #[cfg(test)]
    pub(crate) fn new(headers: HeaderMap, peer: Option<SocketAddr>) -> Caller {
        Caller { headers, peer }
    }
}

impl Deref for Caller {
    type Target = HeaderMap;

    fn deref(&self) -> &HeaderMap {
        &self.headers
    }
}

impl<S> FromRequestParts<S> for Caller
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Caller, Infallible> {
        Ok(Caller {
            headers: parts.headers.clone(),
            peer: parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(peer)| *peer),
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::FromRequestParts as _;
    use axum::http::Request;
    use axum::http::header::AUTHORIZATION;

    use super::*;

    #[tokio::test]
    async fn the_extractor_carries_the_headers_and_the_planted_peer() {
        let peer: SocketAddr = "127.0.0.1:50000".parse().expect("a socket address");
        let mut request = Request::builder()
            .header(AUTHORIZATION, "Bearer x")
            .body(Body::empty())
            .expect("static request parts are valid");
        request.extensions_mut().insert(ConnectInfo(peer));
        let (mut parts, _body) = request.into_parts();

        let Ok(caller) = Caller::from_request_parts(&mut parts, &()).await;

        assert_eq!(caller.peer(), Some(peer));
        assert_eq!(
            caller
                .get(AUTHORIZATION)
                .map(axum::http::HeaderValue::as_bytes),
            Some(b"Bearer x".as_slice()),
            "the extractor derefs to the request's header map"
        );
    }

    #[tokio::test]
    async fn a_missing_peer_extracts_as_none_rather_than_rejecting() {
        let request = Request::builder()
            .body(Body::empty())
            .expect("static request parts are valid");
        let (mut parts, _body) = request.into_parts();

        let Ok(caller) = Caller::from_request_parts(&mut parts, &()).await;

        assert_eq!(caller.peer(), None);
        assert!(caller.is_empty());
    }
}

#[cfg(test)]
mod keyless_loopback_tests {
    //! Rule 3 of [`crate::check_auth`] through the real router: a
    //! credential-free loopback caller is admitted on every route class
    //! unless its Fetch Metadata marks a cross-origin page, a wrong
    //! bearer is still refused on loopback, a LAN or peerless caller
    //! earns no trust, and `trust_loopback = false` restores strict
    //! bearer auth.

    use std::net::SocketAddr;

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{Method, Request, StatusCode};
    use gateway_config::Config;
    use tower::ServiceExt;

    use crate::test_support::app_state;
    use crate::{AppState, build_router};

    const LOOPBACK: &str = "127.0.0.1:50000";
    const LAN: &str = "198.51.100.7:44821";

    /// A state whose key is `test-token`, with `trust_loopback` set as
    /// given (absent means the default).
    fn state(trust_loopback: Option<bool>) -> AppState {
        let trust =
            trust_loopback.map_or(String::new(), |trust| format!("trust_loopback = {trust}\n"));
        let config = Config::from_toml_str(&format!(
            "config-version = 2\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n{trust}"
        ))
        .expect("config parses");
        app_state(config, None)
    }

    /// One route from each class - the inference surface, the any-source
    /// admin surface, and the walled shutdown route - with the status an
    /// admitted empty-bodied request earns on it.
    fn routes() -> Vec<(Method, &'static str, StatusCode)> {
        vec![
            (Method::GET, "/v1/models", StatusCode::OK),
            (Method::GET, "/admin/status", StatusCode::OK),
            (Method::POST, "/shutdown", StatusCode::ACCEPTED),
        ]
    }

    /// Sends one empty-bodied request with the given peer planted (or
    /// none), an optional `Authorization` header value, and an optional
    /// `Sec-Fetch-Site` value.
    async fn send(
        state: &AppState,
        method: Method,
        path: &str,
        peer: Option<&str>,
        authorization: Option<&str>,
        sec_fetch_site: Option<&str>,
    ) -> StatusCode {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(authorization) = authorization {
            builder = builder.header(AUTHORIZATION, authorization);
        }
        if let Some(site) = sec_fetch_site {
            builder = builder.header("sec-fetch-site", site);
        }
        let mut request = builder
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

    #[tokio::test]
    async fn a_keyless_loopback_caller_is_admitted_on_every_route_class() {
        let state = state(None);
        for (method, path, admitted) in routes() {
            let status = send(&state, method.clone(), path, Some(LOOPBACK), None, None).await;
            assert_eq!(
                status, admitted,
                "{method} {path}: a loopback peer with no credential and no fetch metadata is admitted"
            );
        }
    }

    #[tokio::test]
    async fn same_origin_and_none_fetch_metadata_keep_the_keyless_loopback_caller_admitted() {
        let state = state(None);
        for site in ["same-origin", "none"] {
            for (method, path, admitted) in routes() {
                let status = send(
                    &state,
                    method.clone(),
                    path,
                    Some(LOOPBACK),
                    None,
                    Some(site),
                )
                .await;
                assert_eq!(
                    status, admitted,
                    "{method} {path}: Sec-Fetch-Site: {site} is the SPA or a typed URL"
                );
            }
        }
    }

    #[tokio::test]
    async fn cross_origin_fetch_metadata_refuses_the_keyless_loopback_caller() {
        let state = state(None);
        for site in ["cross-site", "same-site", "not-a-site"] {
            for (method, path, _admitted) in routes() {
                let status = send(
                    &state,
                    method.clone(),
                    path,
                    Some(LOOPBACK),
                    None,
                    Some(site),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{method} {path}: Sec-Fetch-Site: {site} marks a page riding the loopback peer"
                );
            }
        }
    }

    /// The CSRF shape the rule exists to stop, on the inference route a
    /// page would actually target: a well-formed chat request from a
    /// loopback peer is refused when `Sec-Fetch-Site: cross-site` marks it
    /// as another origin's page, and reaches routing (404: no such model)
    /// when the same request carries no fetch metadata.
    #[tokio::test]
    async fn cross_site_fetch_metadata_refuses_a_keyless_loopback_chat_completion() {
        let state = state(None);
        let chat = |site: Option<&'static str>| {
            let mut builder = Request::builder()
                .method(Method::POST)
                .uri("/v1/chat/completions")
                .header("content-type", "application/json");
            if let Some(site) = site {
                builder = builder.header("sec-fetch-site", site);
            }
            let mut request = builder
                .body(Body::from(
                    r#"{"model":"no-such-model","messages":[{"role":"user","content":"ping"}]}"#,
                ))
                .expect("static request parts are valid");
            let peer: SocketAddr = LOOPBACK.parse().expect("a socket address");
            request.extensions_mut().insert(ConnectInfo(peer));
            request
        };
        let router = build_router(state.clone(), None);

        let refused = router
            .clone()
            .oneshot(chat(Some("cross-site")))
            .await
            .expect("the router is infallible")
            .status();
        assert_eq!(
            refused,
            StatusCode::UNAUTHORIZED,
            "a cross-site page riding the loopback peer never reaches routing"
        );

        let admitted = router
            .oneshot(chat(None))
            .await
            .expect("the router is infallible")
            .status();
        assert_eq!(
            admitted,
            StatusCode::NOT_FOUND,
            "the same request with no fetch metadata passes auth and is judged by routing"
        );
    }

    #[tokio::test]
    async fn a_wrong_bearer_on_loopback_stays_401() {
        let state = state(None);
        for (method, path, _admitted) in routes() {
            let status = send(
                &state,
                method.clone(),
                path,
                Some(LOOPBACK),
                Some("Bearer wrong"),
                None,
            )
            .await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {path}: presenting a wrong credential is not the same as presenting none"
            );
        }
    }

    #[tokio::test]
    async fn a_keyless_lan_peer_is_refused_everywhere() {
        let state = state(None);
        for (method, path, _admitted) in routes() {
            let status = send(&state, method.clone(), path, Some(LAN), None, None).await;
            // The walled shutdown route refuses a LAN peer before auth runs.
            let refused = if path == "/shutdown" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            };
            assert_eq!(
                status, refused,
                "{method} {path}: loopback trust never reaches a LAN peer"
            );
        }
    }

    #[tokio::test]
    async fn a_keyless_caller_with_no_peer_address_is_refused_everywhere() {
        let state = state(None);
        for (method, path, _admitted) in routes() {
            let status = send(&state, method.clone(), path, None, None, None).await;
            let refused = if path == "/shutdown" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            };
            assert_eq!(
                status, refused,
                "{method} {path}: no recorded peer means no trust"
            );
        }
    }

    #[tokio::test]
    async fn trust_loopback_false_requires_the_key_from_a_loopback_peer() {
        let state = state(Some(false));
        for (method, path, admitted) in routes() {
            let keyless = send(&state, method.clone(), path, Some(LOOPBACK), None, None).await;
            assert_eq!(
                keyless,
                StatusCode::UNAUTHORIZED,
                "{method} {path}: the opt-out restores strict bearer auth"
            );
            let keyed = send(
                &state,
                method.clone(),
                path,
                Some(LOOPBACK),
                Some("Bearer test-token"),
                None,
            )
            .await;
            assert_eq!(
                keyed, admitted,
                "{method} {path}: the key still admits under the opt-out"
            );
        }
    }

    #[tokio::test]
    async fn an_explicit_trust_loopback_true_matches_the_default() {
        let state = state(Some(true));
        let status = send(
            &state,
            Method::GET,
            "/admin/status",
            Some(LOOPBACK),
            None,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
}
