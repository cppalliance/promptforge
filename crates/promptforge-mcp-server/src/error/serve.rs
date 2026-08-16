//! The transport-start failure and its classification.

use std::fmt;
use std::net::SocketAddr;

/// A transport that would not start, or that stopped abnormally.
///
/// Opaque and crate-internal: the representation is private, and the type itself
/// is off the public API because the serve functions are crate-private and the
/// public boot entry surfaces this failure only through the opaque
/// [`RunError`](crate::RunError). A caller within the crate reads the underlying
/// cause through [`std::error::Error::source`] rather than matching a variant.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct ServeError {
    repr: ServeErrorRepr,
}

/// The private representation of a [`ServeError`]. Kept out of the public
/// surface so no dependency error type or bind address is exposed and the shape
/// stays free to change behind [`ServeError::kind`].
#[derive(Debug)]
enum ServeErrorRepr {
    /// `[server].api_key` was absent and the streamable-HTTP transport has no
    /// shared bearer to check. Refused before the socket is bound, since a
    /// `/mcp` served without one would be open to anything that can reach it.
    MissingToken,
    /// The bind address is not a loopback one and `[server].allowed_hosts` was
    /// left empty, so ordinary requests using the machine's DNS name would be
    /// rejected by `Host` validation. Refused before the socket is bound rather
    /// than serving a surface whose reachable-host policy silently contradicts
    /// its bind.
    AllowedHosts { addr: SocketAddr },
    /// The configured socket could not be bound. Carries the address and the
    /// underlying I/O error as its source.
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    /// The HTTP accept loop stopped with an error, kept as its source.
    Http { source: std::io::Error },
    /// The stdio session did not complete its handshake, or ended abnormally.
    /// The cause is carried erased as its source, so the chain is readable
    /// through [`std::error::Error::source`] while no dependency's error type
    /// reaches this crate's public surface.
    Stdio {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ServeError {
    /// `[server].api_key` was absent and the HTTP transport has no bearer.
    pub(crate) fn missing_token() -> ServeError {
        ServeError {
            repr: ServeErrorRepr::MissingToken,
        }
    }

    /// A non-loopback bind was configured with no explicit allowed-host list.
    pub(crate) fn allowed_hosts(addr: SocketAddr) -> ServeError {
        ServeError {
            repr: ServeErrorRepr::AllowedHosts { addr },
        }
    }

    /// The configured socket could not be bound.
    pub(crate) fn bind(addr: SocketAddr, source: std::io::Error) -> ServeError {
        ServeError {
            repr: ServeErrorRepr::Bind { addr, source },
        }
    }

    /// The HTTP accept loop stopped with an error.
    pub(crate) fn http(source: std::io::Error) -> ServeError {
        ServeError {
            repr: ServeErrorRepr::Http { source },
        }
    }

    /// The stdio session did not complete, or ended abnormally.
    pub(crate) fn stdio(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ServeError {
        ServeError {
            repr: ServeErrorRepr::Stdio {
                source: source.into(),
            },
        }
    }
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            ServeErrorRepr::MissingToken => {
                f.write_str("[server].api_key is required to serve over http")
            }
            ServeErrorRepr::AllowedHosts { addr } => write!(
                f,
                "[server].bind {addr} is not loopback, so [server].allowed_hosts must enumerate the authorities clients reach it by"
            ),
            ServeErrorRepr::Bind { addr, .. } => write!(f, "bind {addr}"),
            ServeErrorRepr::Http { .. } => f.write_str("serve http"),
            ServeErrorRepr::Stdio { .. } => f.write_str("serve stdio"),
        }
    }
}

impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.repr {
            ServeErrorRepr::Bind { source, .. } | ServeErrorRepr::Http { source } => Some(source),
            ServeErrorRepr::Stdio { source } => Some(source.as_ref()),
            ServeErrorRepr::MissingToken | ServeErrorRepr::AllowedHosts { .. } => None,
        }
    }
}

/// A stable, dependency-free classification of a [`ServeError`].
///
/// Crate-internal, and read only by the transport tests that assert which
/// startup refusal or run failure a serve attempt produced; production surfaces
/// serve failures through the opaque [`RunError`](crate::RunError) instead.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub(crate) enum ServeErrorKind {
    /// `[server].api_key` was absent and the HTTP transport has no bearer.
    MissingToken,
    /// A non-loopback bind was configured without an explicit allowed-host list.
    AllowedHosts,
    /// The configured socket could not be bound.
    Bind,
    /// The HTTP accept loop stopped with an error.
    Http,
    /// The stdio session did not complete, or ended abnormally.
    Stdio,
}

#[cfg(test)]
impl ServeError {
    /// Classifies the failure without exposing the error's representation.
    #[must_use]
    pub(crate) fn kind(&self) -> ServeErrorKind {
        match &self.repr {
            ServeErrorRepr::MissingToken => ServeErrorKind::MissingToken,
            ServeErrorRepr::AllowedHosts { .. } => ServeErrorKind::AllowedHosts,
            ServeErrorRepr::Bind { .. } => ServeErrorKind::Bind,
            ServeErrorRepr::Http { .. } => ServeErrorKind::Http,
            ServeErrorRepr::Stdio { .. } => ServeErrorKind::Stdio,
        }
    }
}
