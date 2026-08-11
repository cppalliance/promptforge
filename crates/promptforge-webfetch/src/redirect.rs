//! Per-hop redirect policy layered on top of the guarded resolver.
//!
//! The guarded resolver already blocks a connection to an internal address on
//! every hop, because it runs at resolve time. This module adds the URL-level
//! checks the resolver cannot see: it refuses an `https` to `http` downgrade and
//! re-runs [`check_url`] (scheme, userinfo, port, IP-literal classification) on
//! each redirect target, and it caps the number of hops. [`redirect_policy`]
//! builds the [`reqwest::redirect::Policy`] the client installs;
//! [`check_redirect`] is the pure decision it makes on one hop, factored out so
//! it can be tested without a live server.

use reqwest::redirect::Policy;
use url::Url;

use crate::config::FetchConfig;
use crate::error::{FetchError, SafeUrl};
use crate::url_policy::check_url;

/// Decides whether one redirect hop from `previous` to `next` is allowed.
///
/// `previous` is the chain of URLs already requested, as reqwest supplies it,
/// the originating URL first. Its length is the ordinal of the prospective
/// redirect: on the first redirect `previous.len()` is 1 and no hop has yet been
/// followed, so `previous.len() > config.max_redirects()` follows exactly the
/// configured number and refuses the next. The hop is also refused when it
/// downgrades from `https` to `http`, or when [`check_url`] rejects the target's
/// scheme, userinfo, port, or IP-literal address.
///
/// # Errors
/// Returns [`FetchError::RedirectRefused`] naming the from-URL, the to-URL, and
/// the reason for any refused hop.
pub(crate) fn check_redirect(
    previous: &[Url],
    next: &Url,
    config: &FetchConfig,
) -> Result<(), FetchError> {
    let from = previous.last();
    let from_str = from.map_or_else(|| "(origin)".to_string(), ToString::to_string);
    let from_safe = SafeUrl::new(&from_str);
    let to_safe = SafeUrl::new(next.as_str());

    if previous.len() > config.max_redirects() {
        return Err(FetchError::RedirectRefused {
            from: from_safe,
            to: to_safe,
            reason: format!("exceeded max redirects ({})", config.max_redirects()),
        });
    }

    if let Some(from) = from
        && from.scheme() == "https"
        && next.scheme() == "http"
    {
        return Err(FetchError::RedirectRefused {
            from: from_safe,
            to: to_safe,
            reason: "refusing https to http downgrade".to_string(),
        });
    }

    if let Err(err) = check_url(next.as_str(), config) {
        return Err(FetchError::RedirectRefused {
            from: from_safe,
            to: to_safe,
            reason: err.model_facing(),
        });
    }

    Ok(())
}

/// Builds the redirect policy that enforces [`check_redirect`] on every hop.
///
/// A refused hop fails the request with the [`FetchError::RedirectRefused`] as
/// the error source, so the fetch path can recover it and render it. An allowed
/// hop is followed.
#[must_use]
pub(crate) fn redirect_policy(config: FetchConfig) -> Policy {
    Policy::custom(move |attempt| {
        match check_redirect(attempt.previous(), attempt.url(), &config) {
            Ok(()) => attempt.follow(),
            Err(err) => attempt.error(err),
        }
    })
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::check_redirect;
    use crate::config::FetchConfig;
    use crate::error::FetchError;

    /// Parses `s` into a [`Url`], panicking with context on failure.
    fn url(s: &str) -> Url {
        Url::parse(s).expect("test url must parse")
    }

    #[test]
    fn refuses_https_to_http_downgrade() {
        let cfg = FetchConfig::builder()
            .allow_http(true)
            .build()
            .expect("valid config");
        let previous = [url("https://example.com/")];
        let next = url("http://example.com/");
        let err = check_redirect(&previous, &next, &cfg)
            .expect_err("an https to http downgrade must be refused");
        match err {
            FetchError::RedirectRefused { reason, .. } => {
                assert!(reason.contains("downgrade"), "reason was: {reason}");
            }
            other => panic!("expected RedirectRefused, got {other:?}"),
        }
    }

    #[test]
    fn refuses_redirect_to_ip_literal() {
        let cfg = FetchConfig::builder()
            .allow_http(true)
            .build()
            .expect("valid config");
        let previous = [url("http://example.com/")];
        let next = url("http://127.0.0.1/");
        let err = check_redirect(&previous, &next, &cfg)
            .expect_err("a redirect to an ip literal must be refused");
        assert!(matches!(err, FetchError::RedirectRefused { .. }));
    }

    /// Redirect targets with userinfo, a blocked port, or a disallowed scheme
    /// are each refused.
    #[test]
    fn refuses_redirect_targets_by_url_policy() {
        let cfg = FetchConfig::default();
        let previous = [url("https://example.com/")];
        for next in [
            "https://user:pass@example.com/",
            "https://example.com:8080/",
            "ftp://example.com/",
        ] {
            let err = check_redirect(&previous, &url(next), &cfg)
                .expect_err("a policy-violating redirect target must be refused");
            assert!(
                matches!(err, FetchError::RedirectRefused { .. }),
                "for {next}"
            );
        }
    }

    /// Redirect targets given as IP literals in alternate IPv4 encodings and
    /// embedded IPv6 forms are each refused before any connection.
    #[test]
    fn refuses_encoded_ip_literal_redirect_targets() {
        // An http origin so the refusal is the literal classification, not the
        // https-to-http downgrade guard.
        let cfg = FetchConfig::builder()
            .allow_http(true)
            .build()
            .expect("valid config");
        let previous = [url("http://example.com/")];
        for next in [
            "http://0177.0.0.1/",         // octal IPv4
            "http://2130706433/",         // decimal (integer) IPv4
            "http://127.1/",              // short-form IPv4
            "http://[::1]/",              // IPv6 loopback literal
            "http://[::ffff:127.0.0.1]/", // IPv4-mapped IPv6
            "http://[::127.0.0.1]/",      // IPv4-compatible IPv6
        ] {
            let err = check_redirect(&previous, &url(next), &cfg)
                .expect_err("an encoded IP-literal redirect target must be refused");
            assert!(
                matches!(err, FetchError::RedirectRefused { .. }),
                "for {next}"
            );
        }
    }

    /// The two redirect-cap boundaries: exactly the cap is followed, and one
    /// past it is refused.
    #[test]
    fn redirect_cap_boundaries() {
        let cfg = FetchConfig::builder()
            .max_redirects(2)
            .build()
            .expect("valid config");
        // previous.len() == 2 == cap: the prospective hop is allowed.
        let at_cap = [url("https://a.example/"), url("https://b.example/")];
        assert!(
            check_redirect(&at_cap, &url("https://c.example/"), &cfg).is_ok(),
            "exactly the cap must be followed"
        );
        // previous.len() == 3 > cap: refused.
        let over_cap = [
            url("https://a.example/"),
            url("https://b.example/"),
            url("https://c.example/"),
        ];
        let err = check_redirect(&over_cap, &url("https://d.example/"), &cfg)
            .expect_err("one past the cap must be refused");
        match err {
            FetchError::RedirectRefused { reason, .. } => {
                assert!(reason.contains("max redirects"), "reason was: {reason}");
            }
            other => panic!("expected RedirectRefused, got {other:?}"),
        }
    }

    /// A zero cap refuses the very first redirect.
    #[test]
    fn zero_cap_refuses_first_redirect() {
        let cfg = FetchConfig::builder()
            .max_redirects(0)
            .build()
            .expect("valid config");
        let previous = [url("https://a.example/")];
        let err = check_redirect(&previous, &url("https://b.example/"), &cfg)
            .expect_err("a zero cap must refuse the first redirect");
        assert!(matches!(err, FetchError::RedirectRefused { .. }));
    }

    #[test]
    fn allows_ordinary_https_hop() {
        let cfg = FetchConfig::default();
        let previous = [url("https://example.com/")];
        let next = url("https://example.com/next");
        assert!(check_redirect(&previous, &next, &cfg).is_ok());
    }
}
