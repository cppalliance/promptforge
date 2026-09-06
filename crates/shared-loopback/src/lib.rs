//! Shared loopback request checks for PromptForge servers.
//!
//! Two middlewares form the wall. [`require_loopback`] refuses any request
//! whose peer address is not loopback. [`require_loopback_host`] refuses
//! any request whose authority is not the bound loopback socket, closing
//! DNS rebinding. The config-ui crate wraps its SPA asset routes with the
//! peer check (re-exporting it as its own public surface), and the gateway
//! applies the peer check to its admin config endpoints and the host check
//! to its whole loopback-bound surface, so each check exists in exactly one
//! place. The crate is deliberately tiny -
//! axum is its only dependency - because the gateway needs the wall in
//! every build, including headless builds that never compile the
//! config-ui crate and its embedded-asset machinery.
//!
//! WebSocket Origin policy stays explicit and product-specific:
//! [`gateway_loopback_origin_allowed`] admits native clients or HTTP loopback
//! origins, while [`workshop_same_origin_authority_allowed`] requires browser
//! origins to match the Workshop request authority.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::http::header::HOST;
use axum::http::uri::Authority;
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
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| *peer);
    if is_loopback_peer(peer) {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// Whether `peer` is a loopback peer address.
///
/// This is the one peer predicate behind [`require_loopback`], exposed so
/// a caller that keeps the peer address instead of the request (the
/// gateway's keyless-loopback auth rule) asks the same question rather
/// than spelling its own. `None` - no `ConnectInfo` was recorded - fails
/// closed as non-loopback, exactly as the middleware does.
///
/// # Examples
/// ```
/// use std::net::SocketAddr;
///
/// let loopback: SocketAddr = "127.0.0.1:50000".parse()?;
/// let lan: SocketAddr = "198.51.100.7:44821".parse()?;
/// assert!(shared_loopback::is_loopback_peer(Some(loopback)));
/// assert!(!shared_loopback::is_loopback_peer(Some(lan)));
/// assert!(!shared_loopback::is_loopback_peer(None));
/// # Ok::<(), std::net::AddrParseError>(())
/// ```
#[must_use]
pub fn is_loopback_peer(peer: Option<SocketAddr>) -> bool {
    peer.is_some_and(|peer| peer.ip().is_loopback())
}

/// Refuses any request whose authority is not the bound loopback socket
/// with `403 Forbidden`, applied through
/// [`axum::middleware::from_fn_with_state`] with the bound address as the
/// state.
///
/// This is the DNS-rebinding sibling of [`require_loopback`]: a page on a
/// rebound hostname reaches a loopback server with same-origin fetch
/// metadata, but its requests still carry the attacker's name as the
/// authority, the one signal rebinding cannot forge. While the server is
/// bound to a loopback address, the only admitted authorities are the
/// socket's literal form (`127.0.0.1:port` or `[::1]:port`) and
/// `localhost:port` - plus the port-elided bare forms on a port-80 bind,
/// since http clients omit the default port. A server bound to a
/// non-loopback address has no
/// loopback allowlist to enforce, so every request passes: the operator
/// chose network exposure, and refusing non-loopback authorities would
/// break the very clients that bind exists for.
///
/// The URI authority (HTTP/2, absolute-form) wins over the `Host` header.
/// A request naming no authority at all fails closed with `403 Forbidden`:
/// browsers, the house's HTTP clients, and the connection-file health
/// probe all send the bound address as `Host`, so an authority-less
/// request is nothing the wall was built to admit. No route is exempt,
/// `/health` included, which keeps the probe honest against the same check
/// a browser must pass.
pub async fn require_loopback_host(
    State(bound): State<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !bound.ip().is_loopback() {
        return next.run(request).await;
    }
    let authority = request
        .uri()
        .authority()
        .map(axum::http::uri::Authority::as_str)
        .or_else(|| {
            request
                .headers()
                .get(HOST)
                .and_then(|value| value.to_str().ok())
        });
    match authority {
        Some(authority) if authority_allowed(authority, bound) => next.run(request).await,
        _ => StatusCode::FORBIDDEN.into_response(),
    }
}

/// Whether `authority` names the bound loopback socket: its literal
/// `ip:port` form (`[::1]:port` for IPv6) or `localhost:port`, compared
/// ASCII case-insensitively. A client that elides the default http port
/// still names the socket, so a port-80 bind also admits the bare forms
/// (the bare IP, bracketed for IPv6, and bare `localhost`).
fn authority_allowed(authority: &str, bound: SocketAddr) -> bool {
    authority.eq_ignore_ascii_case(bound.to_string().as_str())
        || authority.eq_ignore_ascii_case(format!("localhost:{}", bound.port()).as_str())
        || (bound.port() == 80
            && (authority.eq_ignore_ascii_case(bare_host(bound).as_str())
                || authority.eq_ignore_ascii_case("localhost")))
}

/// The bound address's host without the port: the bare IP, bracketed for
/// IPv6, as an authority eliding the default port renders it.
fn bare_host(bound: SocketAddr) -> String {
    match bound.ip() {
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

/// Whether a Gateway WebSocket Origin is allowed.
///
/// An absent Origin denotes a native client and is admitted. A browser Origin
/// must be an exact HTTP origin whose host is a loopback IP address or
/// `localhost`. HTTPS, foreign hosts, paths, queries, and malformed authorities
/// fail closed.
#[must_use]
pub fn gateway_loopback_origin_allowed(origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    parse_http_origin_authority(origin).is_some_and(|authority| {
        let host = authority.host();
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

/// Whether a Workshop WebSocket Origin matches its request authority.
///
/// An absent Origin denotes a native client, but the request authority must
/// still be present and valid. A browser Origin must be an exact HTTP origin
/// whose normalized authority equals the validated request authority. Missing
/// or malformed values and host or port mismatches fail closed.
#[must_use]
pub fn workshop_same_origin_authority_allowed(
    origin: Option<&str>,
    request_authority: Option<&str>,
) -> bool {
    let Some(request_authority) = request_authority.and_then(parse_authority) else {
        return false;
    };
    origin.is_none_or(|origin| {
        parse_http_origin_authority(origin).is_some_and(|origin_authority| {
            same_origin_authority(&origin_authority, &request_authority)
        })
    })
}

/// Compares normalized hosts while preserving explicit port equality.
fn same_origin_authority(left: &Authority, right: &Authority) -> bool {
    left.port_u16() == right.port_u16() && same_authority_host(left.host(), right.host())
}

/// Compares IP hosts by value and domain hosts ASCII case-insensitively.
fn same_authority_host(left: &str, right: &str) -> bool {
    let parse_ip = |host: &str| {
        host.strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host)
            .parse::<std::net::IpAddr>()
            .ok()
    };
    match (parse_ip(left), parse_ip(right)) {
        (Some(left), Some(right)) => left == right,
        (None, None) => left.eq_ignore_ascii_case(right),
        _ => false,
    }
}

/// Parses an exact HTTP origin and returns its authority.
fn parse_http_origin_authority(origin: &str) -> Option<Authority> {
    let (scheme, authority) = origin.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    parse_authority(authority)
}

/// Parses an authority and rejects ports outside the `u16` range.
fn parse_authority(authority: &str) -> Option<Authority> {
    let port = if let Some(bracketed) = authority.strip_prefix('[') {
        let close = bracketed.find(']')?;
        match &bracketed[close + 1..] {
            "" => None,
            suffix => Some(suffix.strip_prefix(':')?),
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) if !host.is_empty() && !port.contains(':') => Some(port),
            Some(_) => return None,
            None => None,
        }
    };
    if authority.contains('@')
        || port.is_some_and(|port| port.is_empty() || port.parse::<u16>().is_err())
    {
        return None;
    }
    let authority = authority.parse::<Authority>().ok()?;
    if authority.host().is_empty() {
        return None;
    }
    Some(authority)
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

    /// A one-route router with the host wall applied for `bound`,
    /// mirroring how the gateway layers it over its whole surface.
    fn host_guarded_router(bound: SocketAddr) -> Router {
        Router::new().route("/", get(|| async { "ok" })).layer(
            axum::middleware::from_fn_with_state(bound, require_loopback_host),
        )
    }

    /// Sends one request through the host wall with the given `Host`
    /// header (or none at all), against a server bound at `bound`.
    async fn host_status_for(bound: &str, host: Option<&str>) -> StatusCode {
        host_status(bound, "/", host).await
    }

    /// [`host_status_for`] with an explicit request URI, so absolute-form
    /// URIs can carry an authority the `Host` header disagrees with.
    async fn host_status(bound: &str, uri: &str, host: Option<&str>) -> StatusCode {
        let bound: SocketAddr = bound.parse().expect("a socket address");
        let mut builder = HttpRequest::builder().uri(uri);
        if let Some(host) = host {
            builder = builder.header(HOST, host);
        }
        let request = builder
            .body(Body::empty())
            .expect("static request parts are valid");
        host_guarded_router(bound)
            .oneshot(request)
            .await
            .expect("the router is infallible")
            .status()
    }

    #[tokio::test]
    async fn the_bound_ipv4_authority_is_admitted() {
        assert_eq!(
            host_status_for("127.0.0.1:8081", Some("127.0.0.1:8081")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn the_bound_ipv6_authority_is_admitted() {
        assert_eq!(
            host_status_for("[::1]:8081", Some("[::1]:8081")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn localhost_with_the_bound_port_is_admitted() {
        for bound in ["127.0.0.1:8081", "[::1]:8081"] {
            assert_eq!(
                host_status_for(bound, Some("localhost:8081")).await,
                StatusCode::OK,
                "localhost:{bound}'s port names the bound socket"
            );
        }
    }

    #[tokio::test]
    async fn the_authority_comparison_is_case_insensitive() {
        assert_eq!(
            host_status_for("127.0.0.1:8081", Some("LOCALHOST:8081")).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_foreign_authority_is_refused_with_403() {
        for host in ["attacker.com", "attacker.com:8081"] {
            assert_eq!(
                host_status_for("127.0.0.1:8081", Some(host)).await,
                StatusCode::FORBIDDEN,
                "a rebound hostname is refused even on the bound port: {host}"
            );
        }
    }

    #[tokio::test]
    async fn a_loopback_authority_on_the_wrong_port_is_refused() {
        assert_eq!(
            host_status_for("127.0.0.1:8081", Some("127.0.0.1:9999")).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn a_missing_authority_fails_closed_with_403() {
        assert_eq!(
            host_status_for("127.0.0.1:8081", None).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn the_absolute_form_uri_authority_wins_over_the_host_header() {
        assert_eq!(
            host_status(
                "127.0.0.1:8081",
                "http://127.0.0.1:8081/",
                Some("attacker.com")
            )
            .await,
            StatusCode::OK,
            "the request line's authority is the addressed one"
        );
        assert_eq!(
            host_status(
                "127.0.0.1:8081",
                "http://attacker.com:8081/",
                Some("127.0.0.1:8081")
            )
            .await,
            StatusCode::FORBIDDEN,
            "a foreign absolute-form authority is refused despite a loopback Host"
        );
    }

    #[tokio::test]
    async fn a_default_port_bind_admits_the_port_elided_authority() {
        for host in ["127.0.0.1", "localhost", "LOCALHOST"] {
            assert_eq!(
                host_status_for("127.0.0.1:80", Some(host)).await,
                StatusCode::OK,
                "http elides the default port: {host}"
            );
        }
        assert_eq!(
            host_status_for("[::1]:80", Some("[::1]")).await,
            StatusCode::OK,
            "the bracketed bare IPv6 host names the bound socket"
        );
        // Elision is admitted only on the default port.
        assert_eq!(
            host_status_for("127.0.0.1:8081", Some("127.0.0.1")).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn a_non_loopback_bind_admits_any_authority() {
        assert_eq!(
            host_status_for("0.0.0.0:8081", Some("gateway.lan:8081")).await,
            StatusCode::OK,
            "a LAN server has no loopback allowlist to enforce"
        );
        assert_eq!(
            host_status_for("0.0.0.0:8081", None).await,
            StatusCode::OK,
            "even an authority-less request passes a non-loopback bind"
        );
    }

    #[test]
    fn gateway_origin_admits_native_clients_and_http_loopback() {
        assert!(gateway_loopback_origin_allowed(None));
        for origin in [
            "http://127.0.0.1",
            "http://127.5.0.1:8081",
            "http://localhost:8081",
            "http://LOCALHOST:8081",
            "http://[::1]:8081",
        ] {
            assert!(
                gateway_loopback_origin_allowed(Some(origin)),
                "{origin} must be admitted"
            );
        }
    }

    #[test]
    fn gateway_origin_refuses_non_http_foreign_and_malformed_values() {
        for origin in [
            "https://localhost:8081",
            "http://192.168.1.10:8081",
            "http://localhost.evil.example:8081",
            "file:///etc/passwd",
            "http://localhost:bad",
            "http://localhost:8081/path",
            "null",
            "",
        ] {
            assert!(
                !gateway_loopback_origin_allowed(Some(origin)),
                "{origin} must be refused"
            );
        }
    }

    #[test]
    fn workshop_origin_admits_native_clients_with_valid_request_authority() {
        assert!(workshop_same_origin_authority_allowed(
            None,
            Some("127.0.0.1:7910")
        ));
        assert!(!workshop_same_origin_authority_allowed(None, None));
        assert!(!workshop_same_origin_authority_allowed(
            None,
            Some("localhost:bad")
        ));
    }

    #[test]
    fn workshop_origin_requires_matching_normalized_authorities() {
        for (origin, authority) in [
            ("http://127.0.0.1:7910", "127.0.0.1:7910"),
            ("http://localhost:7910", "LOCALHOST:7910"),
            ("http://[::1]:7910", "[::1]:7910"),
            ("http://[0:0:0:0:0:0:0:1]:7910", "[::1]:7910"),
        ] {
            assert!(
                workshop_same_origin_authority_allowed(Some(origin), Some(authority)),
                "{origin} must match {authority}"
            );
        }
    }

    #[test]
    fn workshop_origin_refuses_mismatch_wrong_port_and_malformed_values() {
        for (origin, authority) in [
            ("http://127.0.0.1:7910", "localhost:7910"),
            ("http://localhost:7910", "localhost:7911"),
            ("http://[0:0:0:0:0:0:0:1]:7910", "[::1]:7911"),
            ("http://[0:0:0:0:0:0:0:2]:7910", "[::1]:7910"),
            ("http://evil.example:7910", "localhost:7910"),
            ("http://localhost:bad", "localhost:7910"),
            ("http://localhost:7910/path", "localhost:7910"),
            ("null", "localhost:7910"),
            ("", "localhost:7910"),
        ] {
            assert!(
                !workshop_same_origin_authority_allowed(Some(origin), Some(authority)),
                "{origin} must not match {authority}"
            );
        }
    }
}
