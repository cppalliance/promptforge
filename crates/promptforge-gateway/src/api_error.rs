//! Opaque, source-preserving public error types for the gateway facade.
//!
//! Each public error is a thin wrapper over a private representation. The
//! representation carries the underlying cause via [`std::error::Error::source`]
//! and never appears in a public signature, so no dependency type (reqwest,
//! toml, axum, io) leaks into the crate's semver surface. Callers classify
//! failures with the `kind()` method instead of matching private variants.

use promptforge_gateway_config::ConfigError;

use crate::local::LocalError;

/// A startup failure while assembling or serving the gateway.
///
/// Opaque wrapper preserving the underlying cause via `source()`. Classify with
/// [`StartupError::kind`].
///
/// # Examples
/// ```no_run
/// use promptforge_gateway::{ProfileName, ServeOptions, StartupErrorKind, run};
/// use std::path::PathBuf;
///
/// # fn demo() {
/// let options = ServeOptions::new(
///     PathBuf::from("/etc/promptforge/gateway.toml"),
///     ProfileName::parse("dev").unwrap(),
/// );
/// if let Err(err) = run(&options) {
///     assert!(matches!(err.kind(), StartupErrorKind::Config | StartupErrorKind::Bind));
/// }
/// # }
/// ```
#[non_exhaustive]
pub struct StartupError(StartupRepr);

/// The classification of a [`StartupError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartupErrorKind {
    /// Loading or validating the configuration failed.
    Config,
    /// Provisioning or starting local model children failed.
    Provisioning,
    /// Binding the listener failed.
    Bind,
    /// Serving requests failed.
    Serve,
    /// Starting the hosted workshop server failed. Produced only by builds
    /// with the `workshop` feature.
    Workshop,
}

#[derive(Debug, thiserror::Error)]
enum StartupRepr {
    #[error("configuration error")]
    Config(#[source] ConfigError),
    #[error("local provisioning error")]
    Provisioning(#[source] LocalError),
    #[error("failed to bind the listener")]
    Bind(#[source] std::io::Error),
    #[error("serve error")]
    Serve(#[source] ServeReprSource),
    #[cfg(feature = "workshop")]
    #[error("workshop startup error")]
    Workshop(#[source] promptforge_ws_server::SpawnError),
}

impl StartupError {
    /// Classify this failure without matching a private representation.
    #[must_use]
    pub fn kind(&self) -> StartupErrorKind {
        match self.0 {
            StartupRepr::Config(_) => StartupErrorKind::Config,
            StartupRepr::Provisioning(_) => StartupErrorKind::Provisioning,
            StartupRepr::Bind(_) => StartupErrorKind::Bind,
            StartupRepr::Serve(_) => StartupErrorKind::Serve,
            #[cfg(feature = "workshop")]
            StartupRepr::Workshop(_) => StartupErrorKind::Workshop,
        }
    }

    pub(crate) fn config(err: ConfigError) -> Self {
        StartupError(StartupRepr::Config(err))
    }

    pub(crate) fn provisioning(err: LocalError) -> Self {
        StartupError(StartupRepr::Provisioning(err))
    }

    pub(crate) fn bind(err: std::io::Error) -> Self {
        StartupError(StartupRepr::Bind(err))
    }

    pub(crate) fn serve(err: ServeError) -> Self {
        StartupError(StartupRepr::Serve(err.0))
    }

    #[cfg(feature = "workshop")]
    pub(crate) fn workshop(err: promptforge_ws_server::SpawnError) -> Self {
        StartupError(StartupRepr::Workshop(err))
    }
}

impl std::fmt::Debug for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.0)
    }
}

/// A failure while serving requests on a bound listener.
///
/// Opaque wrapper preserving the underlying cause via `source()`.
#[non_exhaustive]
pub struct ServeError(ServeReprSource);

type ServeReprSource = std::io::Error;

impl ServeError {
    pub(crate) fn io(err: std::io::Error) -> Self {
        ServeError(err)
    }
}

impl std::fmt::Debug for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ServeError").field(&self.0).finish()
    }
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("serve error")
    }
}

impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn startup_error_kind_is_table_driven_and_source_preserving() {
        let cfg = StartupError::config(ConfigError::validation("bad".to_owned()));
        assert_eq!(cfg.kind(), StartupErrorKind::Config);
        assert!(cfg.source().is_some());

        let bind = StartupError::bind(std::io::Error::other("x"));
        assert_eq!(bind.kind(), StartupErrorKind::Bind);
        assert!(bind.source().is_some());
    }
}
