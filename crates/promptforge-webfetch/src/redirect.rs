//! Per-hop redirect policy layered on top of the guarded resolver.
//!
//! The guarded resolver already blocks a connection to an internal address on
//! every hop, because it runs at resolve time. This module adds the URL-level
//! checks the resolver cannot see: it refuses an `https` to `http` downgrade and
//! re-runs [`check_url`] (scheme, userinfo, port, IP literal) on each redirect
//! target, and it caps the number of hops. [`redirect_policy`] builds the
//! [`reqwest::redirect::Policy`] the client installs; [`check_redirect`] is the
//! pure decision it makes on one hop, factored out so it can be tested without a
//! live server.

use reqwest::redirect::Policy;
use url::Url;

use crate::config::FetchConfig;
use crate::error::FetchError;
use crate::url_policy::check_url;

/// Decides whether one redirect hop from `previous` to `next` is allowed.
///
/// `previous` is the chain of URLs already visited (as reqwest supplies it, the
/// originating URL first). The hop is refused when it would exceed
/// `config.max_redirects`, when it downgrades from `https` to `http`, or when
/// [`check_url`] rejects the target's scheme, userinfo, port, or IP literal.
///
/// # Errors
/// Returns [`FetchError::RedirectRefused`] naming the from-URL, the to-URL, and
/// the reason for any refused hop.
pub fn check_redirect(
    previous: &[Url],
    next: &Url,
    config: &FetchConfig,
) -> Result<(), FetchError> {
    let from = previous.last();
    let from_str = from.map_or_else(|| "(origin)".to_string(), ToString::to_string);
    let to_str = next.to_string();

    // reqwest counts the originating URL as the first entry, so the number of
    // hops already taken is `previous.len()`.
    if previous.len() > config.max_redirects {
        return Err(FetchError::RedirectRefused {
            from: from_str,
            to: to_str,
            reason: format!("exceeded max redirects ({})", config.max_redirects),
        });
    }

    if let Some(from) = from {
        if from.scheme() == "https" && next.scheme() == "http" {
            return Err(FetchError::RedirectRefused {
                from: from_str,
                to: to_str,
                reason: "refusing https to http downgrade".to_string(),
            });
        }
    }

    if let Err(err) = check_url(next.as_str(), config) {
        return Err(FetchError::RedirectRefused {
            from: from_str,
            to: to_str,
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
pub fn redirect_policy(config: FetchConfig) -> Policy {
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
        let cfg = FetchConfig {
            // Even with http allowed at the URL level, a downgrade is refused.
            allow_http: true,
            ..FetchConfig::default()
        };
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
        let cfg = FetchConfig {
            allow_http: true,
            ..FetchConfig::default()
        };
        let previous = [url("http://example.com/")];
        let next = url("http://127.0.0.1/");
        let err = check_redirect(&previous, &next, &cfg)
            .expect_err("a redirect to an ip literal must be refused");
        assert!(matches!(err, FetchError::RedirectRefused { .. }));
    }

    #[test]
    fn refuses_when_hops_exceed_cap() {
        let cfg = FetchConfig {
            max_redirects: 2,
            ..FetchConfig::default()
        };
        let previous = [
            url("https://a.example/"),
            url("https://b.example/"),
            url("https://c.example/"),
        ];
        let next = url("https://d.example/");
        let err = check_redirect(&previous, &next, &cfg)
            .expect_err("exceeding the redirect cap must be refused");
        match err {
            FetchError::RedirectRefused { reason, .. } => {
                assert!(reason.contains("max redirects"), "reason was: {reason}");
            }
            other => panic!("expected RedirectRefused, got {other:?}"),
        }
    }

    #[test]
    fn allows_ordinary_https_hop() {
        let cfg = FetchConfig::default();
        let previous = [url("https://example.com/")];
        let next = url("https://example.com/next");
        assert!(check_redirect(&previous, &next, &cfg).is_ok());
    }
}
