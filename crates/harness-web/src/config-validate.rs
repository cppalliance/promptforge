//! The configuration error and the validators behind
//! [`FetchConfigBuilder::build`](super::FetchConfigBuilder::build).
//!
//! Each validator checks one raw builder field against its constraint and
//! returns the validated newtype the policy stores, or the private error
//! representation naming the field and the violated constraint. The public
//! [`ConfigError`] wraps that representation opaquely.

use std::net::IpAddr;
use std::time::Duration;

use ipnet::IpNet;
use reqwest::header::HeaderValue;

use super::{HostAddressException, MAX_REDIRECTS_CEILING, MaxRedirects, UserAgent, canonical_host};

/// An opaque configuration error.
///
/// Its representation is private and free to change. The [`Display`] rendering
/// names the field and the constraint that was violated.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct ConfigError(#[from] ConfigErrorRepr);

/// The private representation behind [`ConfigError`].
#[derive(Debug, thiserror::Error)]
pub(super) enum ConfigErrorRepr {
    /// The user agent is not a legal HTTP header value.
    #[error("user agent is not a valid http header value")]
    UserAgent(#[source] reqwest::header::InvalidHeaderValue),

    /// A limit was zero, which would disable the bound it governs.
    #[error("{field} must be greater than zero")]
    ZeroLimit {
        /// The name of the offending limit.
        field: &'static str,
    },

    /// A limit exceeded its hard operational ceiling.
    #[error("{field} ({value}) exceeds the maximum of {ceiling}")]
    OverCeiling {
        /// The name of the offending limit.
        field: &'static str,
        /// The rejected value.
        value: usize,
        /// The ceiling it exceeded.
        ceiling: usize,
    },

    /// A timeout was zero, which the policy does not allow.
    #[error("{field} must be a positive duration")]
    ZeroTimeout {
        /// The name of the offending timeout.
        field: &'static str,
    },

    /// A timeout exceeded its hard operational ceiling.
    #[error("{field} ({value:?}) exceeds the maximum of {ceiling:?}")]
    TimeoutOverCeiling {
        /// The name of the offending timeout.
        field: &'static str,
        /// The rejected duration.
        value: Duration,
        /// The ceiling it exceeded.
        ceiling: Duration,
    },

    /// A denied-CIDR string did not parse.
    #[error("invalid deny cidr {cidr}")]
    Cidr {
        /// The rejected CIDR text.
        cidr: String,
        /// The parse failure.
        #[source]
        source: ipnet::AddrParseError,
    },

    /// An exact-host exception named an empty or malformed host.
    #[error("invalid exact host {host:?}")]
    Host {
        /// The rejected host text.
        host: String,
    },

    /// The HTTP client could not be built for the validated policy.
    #[error("http client construction failed")]
    ClientBuild(#[source] reqwest::Error),
}

impl ConfigError {
    /// Builds a `ConfigError` from a reqwest client-build failure.
    pub(crate) fn client_build(source: reqwest::Error) -> ConfigError {
        ConfigError(ConfigErrorRepr::ClientBuild(source))
    }
}

/// Validates a `User-Agent` string as a legal HTTP header value.
pub(super) fn validate_user_agent(ua: String) -> Result<UserAgent, ConfigErrorRepr> {
    HeaderValue::from_str(&ua).map_err(ConfigErrorRepr::UserAgent)?;
    Ok(UserAgent(ua))
}

/// Validates a positive limit against a hard ceiling.
pub(super) fn validate_limit(
    field: &'static str,
    value: usize,
    ceiling: usize,
) -> Result<usize, ConfigErrorRepr> {
    if value == 0 {
        return Err(ConfigErrorRepr::ZeroLimit { field });
    }
    if value > ceiling {
        return Err(ConfigErrorRepr::OverCeiling {
            field,
            value,
            ceiling,
        });
    }
    Ok(value)
}

/// Validates the redirect cap against its ceiling; zero is permitted.
pub(super) fn validate_redirects(value: usize) -> Result<MaxRedirects, ConfigErrorRepr> {
    if value > MAX_REDIRECTS_CEILING {
        return Err(ConfigErrorRepr::OverCeiling {
            field: "max_redirects",
            value,
            ceiling: MAX_REDIRECTS_CEILING,
        });
    }
    Ok(MaxRedirects(value))
}

/// Validates a timeout as strictly positive and within its ceiling.
pub(super) fn validate_timeout(
    field: &'static str,
    value: Duration,
    ceiling: Duration,
) -> Result<Duration, ConfigErrorRepr> {
    if value.is_zero() {
        return Err(ConfigErrorRepr::ZeroTimeout { field });
    }
    if value > ceiling {
        return Err(ConfigErrorRepr::TimeoutOverCeiling {
            field,
            value,
            ceiling,
        });
    }
    Ok(value)
}

/// Parses and validates the denied-CIDR strings into networks.
pub(super) fn validate_deny_cidrs(cidrs: Vec<String>) -> Result<Vec<IpNet>, ConfigErrorRepr> {
    let mut nets = Vec::with_capacity(cidrs.len());
    for cidr in cidrs {
        let net = cidr
            .parse::<IpNet>()
            .map_err(|source| ConfigErrorRepr::Cidr {
                cidr: cidr.clone(),
                source,
            })?;
        if !nets.contains(&net) {
            nets.push(net);
        }
    }
    Ok(nets)
}

/// Canonicalizes and validates one exact-exception host.
///
/// Accepts an IP literal (for a literal-host exception) or a syntactically valid
/// DNS domain. Every other form - empty, whitespace-, slash-, colon-, at-, or
/// query-bearing - is rejected by the URL host parser, which enforces the URL
/// forbidden-host-code-point set. Returns the canonical host to store.
pub(super) fn validate_host(raw: &str) -> Result<String, ConfigErrorRepr> {
    let host = canonical_host(raw);
    if host.is_empty() {
        return Err(ConfigErrorRepr::Host {
            host: raw.to_string(),
        });
    }
    // An IP literal is a legitimate exact-exception host (a literal-host URL).
    if host.parse::<IpAddr>().is_ok() {
        return Ok(host);
    }
    // Otherwise require a valid DNS domain. `url::Host::parse` rejects every
    // forbidden host code point (`:` outside brackets, `@`, `?`, `#`, `/`,
    // whitespace, ...), so `bad:host`, `bad@host`, and `bad?host` are refused,
    // while a non-domain address form is not a valid exact host here.
    match url::Host::parse(&host) {
        Ok(url::Host::Domain(domain)) => Ok(domain),
        _ => Err(ConfigErrorRepr::Host {
            host: raw.to_string(),
        }),
    }
}

/// Canonicalizes and validates the exact host-plus-address exceptions.
pub(super) fn validate_allow_hosts(
    hosts: Vec<(String, IpAddr)>,
) -> Result<Vec<HostAddressException>, ConfigErrorRepr> {
    let mut out: Vec<HostAddressException> = Vec::with_capacity(hosts.len());
    for (raw, addr) in hosts {
        let host = validate_host(&raw)?;
        let entry = HostAddressException { host, addr };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    Ok(out)
}
