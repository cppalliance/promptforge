//! URL admission policy applied before any network access.
//!
//! [`check_url`] parses a model-supplied URL and rejects anything the policy
//! forbids (a bad scheme, embedded userinfo, a disallowed port, or a
//! non-global IP-literal host) before a request is ever made. It runs at the top
//! of the fetch path and again on every redirect target, so a rejected URL
//! never reaches the network.
//!
//! IP-literal hosts are the SSRF-critical case: a literal-host URL is connected
//! to directly and never passes through the guarded DNS resolver, so the literal
//! is classified here against the same address policy. `allow_ip_literals`
//! grants literal *syntax* only; a blocked address class is refused even when
//! literals are enabled, reachable solely through an exact host-plus-address
//! exception.

use std::net::IpAddr;

use url::{Host, Url};

use crate::address::{addr_allowed_for_host, blocked_range};
use crate::config::FetchConfig;
use crate::error::FetchError;

/// Parses `raw` and enforces the URL-admission policy in `config`.
///
/// On success the returned [`Url`] has its fragment dropped (a fragment never
/// travels to the server) and its query left untouched. The checks run before
/// any network access, so a rejected URL costs no request.
///
/// The policy: the scheme must be `https`, or `http` when the config permits
/// it; the URL must carry no userinfo; the effective port must be on the
/// allowlist; and an IP-literal host is refused unless literals are enabled, in
/// which case the literal's address is classified against the address policy and
/// a non-global address is still refused.
///
/// # Errors
/// Returns [`FetchError::InvalidUrl`] if `raw` does not parse;
/// [`FetchError::BlockedScheme`] if the scheme is not permitted;
/// [`FetchError::Userinfo`] if the URL contains a `user:pass@` component;
/// [`FetchError::BlockedPort`] if the effective port is not on the allowlist;
/// [`FetchError::IpLiteral`] if the host is an IP literal and literals are not
/// allowed; and [`FetchError::BlockedAddress`] if an admitted literal names a
/// non-global address.
pub(crate) fn check_url(raw: &str, config: &FetchConfig) -> Result<Url, FetchError> {
    let mut url = Url::parse(raw).map_err(FetchError::InvalidUrl)?;

    let scheme = url.scheme();
    let scheme_ok = scheme == "https" || (scheme == "http" && config.allow_http());
    if !scheme_ok {
        return Err(FetchError::BlockedScheme(scheme.to_string()));
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::Userinfo);
    }

    // The scheme is constrained to http or https above, so the default port is
    // always known. Derive it from the scheme rather than through
    // `port_or_known_default`, whose `None` case cannot occur here.
    let default_port = if scheme == "https" { 443 } else { 80 };
    let port = url.port().unwrap_or(default_port);
    if !config.allow_ports().contains(&port) {
        return Err(FetchError::BlockedPort(port));
    }

    // Classify an IP-literal host against the address policy. Literals bypass
    // the guarded DNS resolver (hyper connects to them directly), so this is the
    // only place a literal destination is admitted. `allow_ip_literals` grants
    // syntax only; the address class is still enforced.
    let literal = match url.host() {
        Some(Host::Ipv4(addr)) => Some(IpAddr::V4(addr)),
        Some(Host::Ipv6(addr)) => Some(IpAddr::V6(addr)),
        Some(Host::Domain(_)) | None => None,
    };
    if let Some(ip) = literal {
        if !config.allow_ip_literals() {
            return Err(FetchError::IpLiteral(ip.to_string()));
        }
        let host = url.host_str().unwrap_or_default().to_string();
        if !addr_allowed_for_host(&host, ip, config) {
            let range =
                blocked_range(ip, config).unwrap_or_else(|| "not globally reachable".to_string());
            return Err(FetchError::BlockedAddress {
                host,
                addr: ip,
                range,
            });
        }
    }

    url.set_fragment(None);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::check_url;
    use crate::config::FetchConfig;
    use crate::error::FetchError;

    /// Rejects `raw` and asserts the returned error matches `want`.
    fn assert_rejected(config: &FetchConfig, raw: &str, want: impl Fn(&FetchError) -> bool) {
        let err = check_url(raw, config)
            .expect_err(&format!("expected {raw} to be rejected before any network"));
        assert!(want(&err), "unexpected error for {raw}: {err}");
    }

    #[test]
    fn rejects_userinfo() {
        assert_rejected(
            &FetchConfig::default(),
            "https://user:pass@example.com/",
            |e| matches!(e, FetchError::Userinfo),
        );
    }

    #[test]
    fn rejects_disallowed_port() {
        assert_rejected(&FetchConfig::default(), "https://example.com:8080/", |e| {
            matches!(e, FetchError::BlockedPort(8080))
        });
    }

    #[test]
    fn rejects_http_by_default() {
        assert_rejected(
            &FetchConfig::default(),
            "http://example.com/",
            |e| matches!(e, FetchError::BlockedScheme(s) if s == "http"),
        );
    }

    #[test]
    fn rejects_ip_literals_in_every_encoding() {
        let cfg = FetchConfig::default();
        for raw in [
            "https://0177.0.0.1/",
            "https://2130706433/",
            "https://[::1]/",
            "https://127.1/",
        ] {
            assert_rejected(&cfg, raw, |e| matches!(e, FetchError::IpLiteral(_)));
        }
    }

    #[test]
    fn rejects_unparseable_url() {
        assert_rejected(&FetchConfig::default(), "not a url", |e| {
            matches!(e, FetchError::InvalidUrl(_))
        });
    }

    #[test]
    fn accepts_ordinary_https_url() {
        let config = FetchConfig::default();
        let url = check_url("https://example.com/path?q=1#frag", &config)
            .expect("an ordinary https url should be accepted");
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.query(), Some("q=1"), "the query must be preserved");
        assert_eq!(url.fragment(), None, "the fragment must be dropped");
    }

    #[test]
    fn allow_http_permits_plain_http() {
        let config = FetchConfig::builder()
            .allow_http(true)
            .build()
            .expect("valid config");
        let url =
            check_url("http://example.com/", &config).expect("http should be allowed when enabled");
        assert_eq!(url.scheme(), "http");
    }

    /// With literals enabled, a public literal is admitted but every non-global
    /// literal class is still refused as a hard blocked address.
    #[test]
    fn ip_literals_enabled_still_block_non_global_classes() {
        let config = FetchConfig::builder()
            .allow_ip_literals(true)
            .build()
            .expect("valid config");

        // A public literal is admitted.
        let url = check_url("https://1.1.1.1/", &config).expect("a public literal is admitted");
        assert_eq!(url.host_str(), Some("1.1.1.1"));

        // Every non-global literal class is refused as a hard blocked address.
        let blocked = [
            "https://127.0.0.1/",          // loopback
            "https://10.0.0.1/",           // private
            "https://169.254.169.254/",    // link-local metadata
            "https://100.64.0.1/",         // CGNAT
            "https://[::1]/",              // v6 loopback
            "https://[::ffff:127.0.0.1]/", // mapped IPv4 loopback
            "https://[::127.0.0.1]/",      // compatible IPv4 loopback
            "https://[64:ff9b::7f00:1]/",  // NAT64 loopback
            "https://[ff02::1]/",          // multicast
        ];
        for raw in blocked {
            assert_rejected(&config, raw, |e| {
                matches!(e, FetchError::BlockedAddress { .. })
            });
        }
    }

    /// An exact host-plus-address exception is the only way to reach a blocked
    /// literal, and only for the exact literal that names it.
    #[test]
    fn ip_literal_reachable_only_through_exact_exception() {
        let loopback: IpAddr = "127.0.0.1".parse().expect("loopback parses");
        let config = FetchConfig::builder()
            .allow_ip_literals(true)
            .allow_host_address("127.0.0.1", loopback)
            .build()
            .expect("valid config");

        let url = check_url("https://127.0.0.1/", &config)
            .expect("the exact literal exception admits its address");
        assert_eq!(url.host_str(), Some("127.0.0.1"));

        // A different blocked literal is not covered by the exception.
        assert_rejected(&config, "https://127.0.0.2/", |e| {
            matches!(e, FetchError::BlockedAddress { .. })
        });
    }
}
