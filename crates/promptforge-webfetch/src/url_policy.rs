//! URL admission policy applied before any network access.
//!
//! [`check_url`] parses a model-supplied URL and rejects anything the policy
//! forbids (a bad scheme, embedded userinfo, a disallowed port, or a bare IP
//! literal) before a request is ever made. It runs at the top of the fetch path
//! so a rejected URL never reaches the network.

use url::{Host, Url};

use crate::config::FetchConfig;
use crate::error::FetchError;

/// Parses `raw` and enforces the URL-admission policy in `config`.
///
/// On success the returned [`Url`] has its fragment dropped (a fragment never
/// travels to the server) and its query left untouched. The checks run before
/// any network access, so a rejected URL costs no request.
///
/// The policy: the scheme must be `https`, or `http` when
/// [`FetchConfig::allow_http`] is set; the URL must carry no userinfo; the
/// effective port must appear in [`FetchConfig::allow_ports`]; and a bare IP
/// literal host is refused unless [`FetchConfig::allow_ip_literals`] is set.
/// Literal forms such as `0177.0.0.1`, `2130706433`, and `127.1` are normalized
/// to an IPv4 host by the parser and caught by the IP-literal check.
///
/// # Errors
/// Returns [`FetchError::InvalidUrl`] if `raw` does not parse;
/// [`FetchError::BlockedScheme`] if the scheme is not permitted;
/// [`FetchError::Userinfo`] if the URL contains a `user:pass@` component;
/// [`FetchError::IpLiteral`] if the host is a bare IP literal and literals are
/// not allowed; and [`FetchError::BlockedPort`] if the effective port is not on
/// the allowlist.
pub fn check_url(raw: &str, config: &FetchConfig) -> Result<Url, FetchError> {
    let mut url = Url::parse(raw).map_err(|source| FetchError::InvalidUrl(source.to_string()))?;

    let scheme = url.scheme();
    let scheme_ok = scheme == "https" || (scheme == "http" && config.allow_http);
    if !scheme_ok {
        return Err(FetchError::BlockedScheme(scheme.to_string()));
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::Userinfo);
    }

    if !config.allow_ip_literals {
        if let Some(Host::Ipv4(addr)) = url.host() {
            return Err(FetchError::IpLiteral(addr.to_string()));
        }
        if let Some(Host::Ipv6(addr)) = url.host() {
            return Err(FetchError::IpLiteral(addr.to_string()));
        }
    }

    // The scheme is constrained to http or https above, so the default port is
    // always known. Derive it directly from the scheme rather than through
    // `port_or_known_default`, whose `None` case cannot occur here and would
    // otherwise force a dead arm with a misleading `BlockedPort(0)` sentinel.
    let default_port = if scheme == "https" { 443 } else { 80 };
    let port = url.port().unwrap_or(default_port);
    if !config.allow_ports.contains(&port) {
        return Err(FetchError::BlockedPort(port));
    }

    url.set_fragment(None);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::check_url;
    use crate::config::FetchConfig;
    use crate::error::FetchError;

    /// Rejects `raw` and asserts the returned error matches `want`.
    fn assert_rejected(raw: &str, want: impl Fn(&FetchError) -> bool) {
        let config = FetchConfig::default();
        let err = check_url(raw, &config)
            .expect_err(&format!("expected {raw} to be rejected before any network"));
        assert!(want(&err), "unexpected error for {raw}: {err}");
    }

    #[test]
    fn rejects_userinfo() {
        assert_rejected("https://user:pass@example.com/", |e| {
            matches!(e, FetchError::Userinfo)
        });
    }

    #[test]
    fn rejects_disallowed_port() {
        assert_rejected("https://example.com:8080/", |e| {
            matches!(e, FetchError::BlockedPort(8080))
        });
    }

    #[test]
    fn rejects_http_by_default() {
        assert_rejected(
            "http://example.com/",
            |e| matches!(e, FetchError::BlockedScheme(s) if s == "http"),
        );
    }

    #[test]
    fn rejects_ip_literals_in_every_encoding() {
        for raw in [
            "https://0177.0.0.1/",
            "https://2130706433/",
            "https://[::1]/",
            "https://127.1/",
        ] {
            assert_rejected(raw, |e| matches!(e, FetchError::IpLiteral(_)));
        }
    }

    #[test]
    fn rejects_unparseable_url() {
        assert_rejected("not a url", |e| matches!(e, FetchError::InvalidUrl(_)));
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
        let config = FetchConfig {
            allow_http: true,
            ..FetchConfig::default()
        };
        let url =
            check_url("http://example.com/", &config).expect("http should be allowed when enabled");
        assert_eq!(url.scheme(), "http");
    }
}
