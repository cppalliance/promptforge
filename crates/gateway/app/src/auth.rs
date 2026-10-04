//! The [`Caller`] extractor: what an authenticated handler knows about who
//! is asking - the request headers and, when the server recorded one, the
//! peer address. [`check_auth`] reads both, so a handler that once
//! extracted a bare `HeaderMap` for auth now extracts a `Caller` and
//! changes nothing else: the extractor derefs to the header map.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::ops::Deref;

#[cfg(feature = "stt")]
use axum::extract::State;
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
#[cfg(feature = "stt")]
use axum::http::header::ORIGIN;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::error::GatewayError;

/// The ambient-credential primitives the auth rules read: the handoff
/// cookie, its session proof, and the Fetch Metadata gates. They ship in
/// every build, while the `/auth` route that mints the cookie ships only
/// with the config surface.
pub(crate) mod primitives;

/// The request headers plus the peer address, as [`check_auth`]
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
    /// [`check_auth`] directly rather than through the router.
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

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Caller, Infallible>> {
        std::future::ready(Ok(Caller {
            headers: parts.headers.clone(),
            peer: parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(peer)| *peer),
        }))
    }
}

/// A [`Caller`] that has already passed [`check_auth`]: extraction runs the
/// three auth rules, so a handler's first line is never `check_auth`.
///
/// The rejection fires while the request parts are extracted, before any
/// body extractor runs: an unauthenticated caller never makes the gateway
/// parse a body, the ordering the speech route once arranged by hand.
pub(crate) struct AuthedCaller(Caller);

impl Deref for AuthedCaller {
    type Target = Caller;

    fn deref(&self) -> &Caller {
        &self.0
    }
}

impl FromRequestParts<AppState> for AuthedCaller {
    type Rejection = GatewayError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<AuthedCaller, GatewayError> {
        let Ok(caller) = Caller::from_request_parts(parts, state).await;
        check_auth(state, &caller).await?;
        Ok(AuthedCaller(caller))
    }
}

/// An [`AuthedCaller`] on a connection the server recorded as a loopback
/// peer: the extractor every handler in the walled admin tier takes.
///
/// `build_router` already mounts that tier behind the shared loopback wall,
/// which refuses a non-loopback peer with a bare 403 before auth runs, so in
/// the assembled router this check never fires. The extractor is the
/// handler's own statement of the tier it belongs to, readable from its
/// signature, and it repeats the wall's question through the same
/// [`shared_loopback::is_loopback_peer`] predicate: a walled handler that
/// is ever mounted without the wall still refuses a LAN or peerless caller
/// with the wall's 403, and refuses before auth, in the wall's order.
pub(crate) struct LoopbackCaller(AuthedCaller);

impl Deref for LoopbackCaller {
    type Target = AuthedCaller;

    fn deref(&self) -> &AuthedCaller {
        &self.0
    }
}

impl FromRequestParts<AppState> for LoopbackCaller {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<LoopbackCaller, Response> {
        let Ok(caller) = Caller::from_request_parts(parts, state).await;
        if !shared_loopback::is_loopback_peer(caller.peer()) {
            return Err(StatusCode::FORBIDDEN.into_response());
        }
        check_auth(state, &caller)
            .await
            .map_err(IntoResponse::into_response)?;
        Ok(LoopbackCaller(AuthedCaller(caller)))
    }
}

#[cfg(test)]
mod secret_tests;

#[cfg(test)]
mod loopback_caller_tests;

/// Authenticates the caller by any one of three rules, in this order.
///
/// 1. A presented bearer token equals the live key.
/// 2. The `/auth` browser handoff's cookie verifies: the cookie is the
///    key's ambient form, accepted anywhere the bearer header is. Two
///    guards shape this path that the bearer path does not need: the
///    proof is recomputed from the process-lifetime salt and the live key
///    (the cookie never holds the key itself), and the request must
///    include Fetch Metadata a cross-origin page cannot strip, since an
///    ambient credential would otherwise answer to any same-site loopback
///    page (ports are not part of a site).
/// 3. Loopback trust: `[server] trust_loopback` is on, the server recorded
///    a loopback peer for the connection, the request presents no
///    `Authorization` header at all, and its Fetch Metadata permits
///    ambient access ([`primitives::fetch_metadata_allows_ambient`]).
///
/// Two edges of rule 3 are deliberate. A presented-but-wrong bearer is
/// refused even on loopback: absence of credentials is what loopback
/// trusts, and a caller presenting wrong ones meant to authenticate - the
/// gateway-discovery-file liveness probe relies on that to detect a stale key.
/// And a request with no recorded peer address receives no trust: it
/// needs a credential, the same fail-closed posture as the loopback wall.
pub(crate) async fn check_auth(state: &AppState, caller: &Caller) -> Result<(), GatewayError> {
    let authorization = caller.get(AUTHORIZATION);
    let presented = authorization
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    let live = state.live.read().await;
    if secret_eq(presented.as_bytes(), live.key.expose().as_bytes()) {
        return Ok(());
    }
    if let Some(cookie) = primitives::presented_cookie_proof(caller)
        && primitives::fetch_metadata_allows_cookie(caller)
        && secret_eq(
            &cookie,
            &primitives::session_token(&state.handoff_salt, live.key.expose().as_bytes()),
        )
    {
        return Ok(());
    }
    if live.trust_loopback
        && authorization.is_none()
        && shared_loopback::is_loopback_peer(caller.peer())
        && primitives::fetch_metadata_allows_ambient(caller)
    {
        return Ok(());
    }
    Err(GatewayError::Unauthorized)
}

/// Constant-time credential comparison.
///
/// Both inputs are hashed to fixed-length SHA-256 digests before comparison, so
/// the comparison operates on equal-length data (no early length-based
/// short-circuit) and leaks neither the configured key's length nor its bytes.
/// The digest comparison uses the `subtle` crate's constant-time primitive.
pub(crate) fn secret_eq(presented: &[u8], configured: &[u8]) -> bool {
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;

    let presented = Sha256::digest(presented);
    let configured = Sha256::digest(configured);
    presented.ct_eq(&configured).into()
}

#[cfg(feature = "stt")]
pub(crate) async fn authorize_stt_route(
    State(state): State<AppState>,
    caller: Caller,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<Response, GatewayError> {
    check_auth(&state, &caller).await?;
    if request.uri().path() == "/v1/realtime" && !gateway_realtime_origin_allowed(&request) {
        return Ok(axum::http::StatusCode::FORBIDDEN.into_response());
    }
    Ok(next.run(request).await)
}

#[cfg(feature = "stt")]
fn gateway_realtime_origin_allowed(request: &axum::extract::Request) -> bool {
    let mut values = request.headers().get_all(ORIGIN).iter();
    let first = values.next();
    if values.next().is_some() {
        return false;
    }
    let origin = match first {
        None => None,
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => return false,
        },
    };
    shared_loopback::gateway_loopback_origin_allowed(origin)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::header::AUTHORIZATION;

    use super::*;

    #[tokio::test]
    async fn the_extractor_holds_the_headers_and_the_planted_peer() {
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
mod keyless_loopback_tests;
