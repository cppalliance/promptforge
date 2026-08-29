//! The crate-private `web_fetch` error type.
//!
//! [`FetchError`] is the single internal error type for every `web_fetch`
//! failure mode: the URL and address policies, redirects, DNS, the size caps,
//! the timeouts, the body read, and the content-type and decoding refusals. It
//! is never public; the `Tool::call` boundary maps it to a narrow `ToolError`
//! through [`FetchError::classify`]. It carries a model-facing rendering that
//! withholds internal detail, and preserves dependency and I/O causes behind
//! `#[source]` fields so a log can inspect the chain.

use std::net::IpAddr;

use promptforge_tools::ToolErrorKind;

/// How the `Tool::call` boundary should treat a [`FetchError`].
///
/// A [`Disposition::SoftOutput`] error is returned to the model as untrusted
/// tool text so it can try a different URL; a [`Disposition::Hard`] error aborts
/// the call with the given [`ToolErrorKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Return the model-facing text as untrusted tool output.
    SoftOutput,
    /// Abort the call with this error kind.
    Hard(ToolErrorKind),
}

/// A URL stored for diagnostics with its secrets redacted.
///
/// `SafeUrl::new` strips userinfo, the query string, and the fragment, keeping
/// only scheme, host, port, and path. Because the redaction happens on
/// construction, both the derived [`Debug`] and the [`Display`] rendering of any
/// [`FetchError`] holding a `SafeUrl` are secret-free regardless of the call
/// site, so a `?secret=...` query in a fetched URL never reaches a log, a Debug
/// dump, or the model-facing error text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SafeUrl(String);

impl SafeUrl {
    /// Builds a redacted URL, dropping userinfo, query, and fragment.
    ///
    /// A string that does not parse as a URL is kept verbatim, since it carries
    /// no parseable query to redact (for example the redirect origin sentinel).
    #[must_use]
    pub(crate) fn new(raw: &str) -> SafeUrl {
        match url::Url::parse(raw) {
            Ok(parsed) => {
                let scheme = parsed.scheme();
                let host = parsed.host_str().unwrap_or_default();
                let mut out = format!("{scheme}://{host}");
                if let Some(port) = parsed.port() {
                    out.push(':');
                    out.push_str(&port.to_string());
                }
                out.push_str(parsed.path());
                SafeUrl(out)
            }
            Err(_) => SafeUrl(raw.to_string()),
        }
    }
}

impl std::fmt::Display for SafeUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// An error produced while fetching a URL.
///
/// The [`Display`](std::fmt::Display) rendering is the log text; the
/// [`FetchError::model_facing`] rendering is what a model should see. They
/// diverge for [`FetchError::BlockedAddress`], whose log text names the resolved
/// address and the range that blocked it while its model-facing text says only
/// that the host is not fetchable. Dependency and I/O causes live behind
/// `#[source]` fields and are never rendered in `Display`, so the printed chain
/// does not repeat itself. Every URL field is a [`SafeUrl`], so no query-string
/// secret survives into `Debug` or `Display`.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchError {
    /// The URL could not be parsed.
    #[error("invalid url")]
    InvalidUrl(#[source] url::ParseError),

    /// The URL's scheme is not permitted by the policy.
    #[error("scheme not allowed: {0}")]
    BlockedScheme(String),

    /// The URL carries userinfo (a `user:pass@` component).
    #[error("url must not contain userinfo")]
    Userinfo,

    /// The URL's effective port is not on the allowlist.
    #[error("port not allowed: {0}")]
    BlockedPort(u16),

    /// The URL's host is a bare IP literal, which the policy forbids.
    #[error("ip literal host not allowed: {0}")]
    IpLiteral(String),

    /// A host (or admitted IP literal) resolved to a blocked address.
    ///
    /// The full [`Display`](std::fmt::Display) names the host, the resolved
    /// address, and the range, for the log. [`FetchError::model_facing`] omits
    /// the address and range so no internal topology reaches the model.
    #[error("host {host} resolved to blocked address {addr} in range {range}")]
    BlockedAddress {
        /// The host that was being resolved.
        host: String,
        /// The resolved address that fell in a blocked range.
        addr: IpAddr,
        /// The CIDR range that blocked the address.
        range: String,
    },

    /// A host resolved to no address the policy would allow.
    #[error("host {host} has no allowed address")]
    NoAllowedAddress {
        /// The host that resolved only to blocked (or zero) addresses.
        host: String,
    },

    /// A redirect hop was refused by the redirect policy.
    #[error("redirect from {from} to {to} refused: {reason}")]
    RedirectRefused {
        /// The URL the redirect came from.
        from: SafeUrl,
        /// The URL the redirect pointed to.
        to: SafeUrl,
        /// Why the hop was refused (downgrade, bad scheme, port, cap, ...).
        reason: String,
    },

    /// The response body exceeded the byte cap.
    #[error("response from {url} exceeds the {limit}-byte size cap")]
    TooLarge {
        /// The URL whose response exceeded the cap.
        url: SafeUrl,
        /// The byte cap that was exceeded.
        limit: usize,
    },

    /// Reading the response body failed mid-stream.
    #[error("failed to read the response body from {url}; try again or use a different URL")]
    BodyRead {
        /// The URL whose body read failed.
        url: SafeUrl,
        /// The underlying transport failure.
        #[source]
        source: reqwest::Error,
    },

    /// DNS resolution for a host failed at the system level.
    #[error("dns resolution failed for {host}")]
    Dns {
        /// The host whose resolution failed.
        host: String,
        /// The underlying resolver failure.
        #[source]
        source: std::io::Error,
    },

    /// The response declared a content type this tool does not return as text.
    #[error(
        "content type {content_type} from {url} cannot be returned as text; try an HTML version of the page or a different URL"
    )]
    UnsupportedContentType {
        /// The URL whose response carried the unsupported type.
        url: SafeUrl,
        /// The unsupported content type, verbatim from the response header.
        content_type: String,
    },

    /// The response carried no `Content-Type` header.
    #[error(
        "response from {url} declared no content type; refusing to guess its format; try a different URL"
    )]
    NoContentType {
        /// The URL whose response carried no content type.
        url: SafeUrl,
    },

    /// The request exceeded its time budget.
    #[error("request to {url} timed out; try again or use a different URL")]
    Timeout {
        /// The URL whose request timed out.
        url: SafeUrl,
    },

    /// The response declared a charset that could not be decoded.
    #[error("response from {url} declared unknown charset {charset}; cannot decode its text")]
    Undecodable {
        /// The URL whose response declared the charset.
        url: SafeUrl,
        /// The unrecognized charset label, verbatim from the response header.
        charset: String,
    },

    /// The target URL returned a non-success HTTP status.
    #[error("HTTP {status} from {url}; try a different URL")]
    HttpStatus {
        /// The URL (after redirects) that returned the error status.
        url: SafeUrl,
        /// The HTTP status code.
        status: u16,
    },
}

impl FetchError {
    /// Renders the error as text safe to return to the model.
    ///
    /// Identical to the [`Display`](std::fmt::Display) output for every variant
    /// except [`FetchError::BlockedAddress`], whose model-facing text omits the
    /// resolved address and the blocking range: the model learns only that the
    /// host is not fetchable, never the internal topology behind it.
    #[must_use]
    pub(crate) fn model_facing(&self) -> String {
        match self {
            FetchError::BlockedAddress { host, .. } => {
                format!("host {host} is not fetchable")
            }
            other => other.to_string(),
        }
    }

    /// Decides how the `Tool::call` boundary should treat this error.
    ///
    /// The match is exhaustive with no wildcard, so a new variant forces an
    /// explicit disposition and kind decision at compile time. Target, network,
    /// content, redirect, and blocked-scheme outcomes are soft (returned as tool
    /// text the model can act on); admission and SSRF refusals, decided before a
    /// useful retry shape exists, are hard invalid-arguments errors.
    #[must_use]
    pub(crate) fn classify(&self) -> Disposition {
        match self {
            FetchError::HttpStatus { .. }
            | FetchError::UnsupportedContentType { .. }
            | FetchError::NoContentType { .. }
            | FetchError::Timeout { .. }
            | FetchError::TooLarge { .. }
            | FetchError::BodyRead { .. }
            | FetchError::Undecodable { .. }
            | FetchError::Dns { .. }
            | FetchError::RedirectRefused { .. }
            | FetchError::BlockedScheme(_) => Disposition::SoftOutput,
            FetchError::InvalidUrl(_)
            | FetchError::Userinfo
            | FetchError::BlockedPort(_)
            | FetchError::IpLiteral(_)
            | FetchError::BlockedAddress { .. }
            | FetchError::NoAllowedAddress { .. } => {
                Disposition::Hard(ToolErrorKind::InvalidArguments)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::net::IpAddr;

    use promptforge_tools::ToolErrorKind;

    use super::{Disposition, FetchError, SafeUrl};

    #[test]
    fn blocked_address_log_keeps_detail_model_facing_hides_it() {
        let addr: IpAddr = "169.254.169.254".parse().expect("test address parses");
        let err = FetchError::BlockedAddress {
            host: "metadata.internal".to_string(),
            addr,
            range: "169.254.0.0/16".to_string(),
        };

        let log = err.to_string();
        assert!(log.contains("169.254.169.254"), "log must name the address");
        assert!(log.contains("169.254.0.0/16"), "log must name the range");

        let facing = err.model_facing();
        assert!(
            !facing.contains("169.254.169.254") && !facing.contains("169.254.0.0/16"),
            "model-facing text must hide the address and range, got: {facing}"
        );
        assert!(
            facing.contains("metadata.internal") && facing.contains("not fetchable"),
            "model-facing text should name the host as not fetchable, got: {facing}"
        );
    }

    /// Builds a real `reqwest::Error` offline: an https-only client rejects an
    /// `http` URL without any network access.
    async fn reqwest_error() -> reqwest::Error {
        reqwest::Client::builder()
            .https_only(true)
            .build()
            .expect("client builds")
            .get("http://example.invalid/")
            .send()
            .await
            .expect_err("an http url must be rejected by an https-only client")
    }

    /// The exhaustive disposition table: every non-`BodyRead` variant. `BodyRead`
    /// is covered by [`body_read_error_is_soft_with_reachable_source`], which
    /// needs a real `reqwest::Error`.
    #[test]
    fn disposition_table() {
        let parse = url::Url::parse("not a url").expect_err("must fail to parse");
        let soft: [FetchError; 9] = [
            FetchError::HttpStatus {
                url: SafeUrl::new("https://u/"),
                status: 404,
            },
            FetchError::UnsupportedContentType {
                url: SafeUrl::new("https://u/"),
                content_type: "application/pdf".into(),
            },
            FetchError::NoContentType {
                url: SafeUrl::new("https://u/"),
            },
            FetchError::Timeout {
                url: SafeUrl::new("https://u/"),
            },
            FetchError::TooLarge {
                url: SafeUrl::new("https://u/"),
                limit: 100,
            },
            FetchError::Undecodable {
                url: SafeUrl::new("https://u/"),
                charset: "x".into(),
            },
            FetchError::Dns {
                host: "h".into(),
                source: std::io::Error::other("resolver down"),
            },
            FetchError::RedirectRefused {
                from: SafeUrl::new("https://a/"),
                to: SafeUrl::new("http://a/"),
                reason: "downgrade".into(),
            },
            FetchError::BlockedScheme("http".into()),
        ];
        for err in &soft {
            assert_eq!(
                err.classify(),
                Disposition::SoftOutput,
                "{err} must be soft"
            );
        }

        let hard: [FetchError; 6] = [
            FetchError::InvalidUrl(parse),
            FetchError::Userinfo,
            FetchError::BlockedPort(22),
            FetchError::IpLiteral("1.2.3.4".into()),
            FetchError::BlockedAddress {
                host: "h".into(),
                addr: "127.0.0.1".parse().expect("loopback parses"),
                range: "127.0.0.0/8".into(),
            },
            FetchError::NoAllowedAddress { host: "h".into() },
        ];
        for err in &hard {
            assert_eq!(
                err.classify(),
                Disposition::Hard(ToolErrorKind::InvalidArguments),
                "{err} must be hard invalid-arguments"
            );
        }
    }

    #[tokio::test]
    async fn body_read_error_is_soft_with_reachable_source() {
        let err = FetchError::BodyRead {
            url: SafeUrl::new("https://u/"),
            source: reqwest_error().await,
        };
        assert_eq!(
            err.classify(),
            Disposition::SoftOutput,
            "BodyRead must be soft"
        );
        assert!(err.source().is_some(), "BodyRead must expose its cause");
    }

    #[test]
    fn url_parse_and_dns_errors_keep_a_reachable_source() {
        let parse = url::Url::parse("not a url").expect_err("must fail to parse");
        let err = FetchError::InvalidUrl(parse);
        assert!(err.source().is_some(), "InvalidUrl must expose its cause");

        let io = std::io::Error::other("resolver down");
        let err = FetchError::Dns {
            host: "h".into(),
            source: io,
        };
        assert!(err.source().is_some(), "Dns must expose its cause");
    }

    /// A `?secret=...` query in a stored URL never reaches Debug, Display, or the
    /// model-facing text of any URL-bearing variant.
    #[test]
    fn secret_query_is_redacted_in_debug_and_display() {
        const SECRET: &str = "supersecrettoken";
        let leaky = format!("https://host.example/path?token={SECRET}#frag");
        let variants: [FetchError; 6] = [
            FetchError::TooLarge {
                url: SafeUrl::new(&leaky),
                limit: 1,
            },
            FetchError::Timeout {
                url: SafeUrl::new(&leaky),
            },
            FetchError::HttpStatus {
                url: SafeUrl::new(&leaky),
                status: 500,
            },
            FetchError::NoContentType {
                url: SafeUrl::new(&leaky),
            },
            FetchError::UnsupportedContentType {
                url: SafeUrl::new(&leaky),
                content_type: "application/pdf".into(),
            },
            FetchError::RedirectRefused {
                from: SafeUrl::new(&leaky),
                to: SafeUrl::new(&leaky),
                reason: "downgrade".into(),
            },
        ];
        for err in &variants {
            let debug = format!("{err:?}");
            let display = err.to_string();
            let facing = err.model_facing();
            assert!(!debug.contains(SECRET), "secret leaked in Debug: {debug}");
            assert!(
                !display.contains(SECRET),
                "secret leaked in Display: {display}"
            );
            assert!(
                !facing.contains(SECRET),
                "secret leaked in model_facing: {facing}"
            );
            // The host and path survive for diagnostics.
            assert!(
                display.contains("host.example"),
                "host must survive: {display}"
            );
        }
    }

    #[test]
    fn safe_url_redacts_userinfo_query_and_fragment() {
        let safe = SafeUrl::new("https://user:pass@host.example:8443/a/b?x=secret#frag");
        assert_eq!(safe.to_string(), "https://host.example:8443/a/b");
    }
}
