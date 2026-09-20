//! WebSocket Origin policy: the product-specific rules deciding which
//! browser origins may open a Gateway or Workshop socket.

use axum::http::uri::Authority;

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
    use super::*;

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
