//! Opaque, source-preserving public error types for the gateway facade.
//!
//! Each public error is a thin wrapper over a private representation. The
//! representation carries the underlying cause via [`std::error::Error::source`]
//! and never appears in a public signature, so no dependency type (reqwest,
//! toml, axum, io) leaks into the crate's semver surface. Callers classify
//! failures with the `kind()` method instead of matching private variants.

use crate::error::ConfigError as ConfigErrorRepr;
use crate::local::LocalError;

/// A configuration load or validation failure.
///
/// Opaque wrapper: the underlying `toml`, `io`, and validation detail are kept
/// as a private `source()`; classify with [`ConfigError::kind`].
///
/// # Examples
/// ```
/// use promptforge_gateway::{Config, ConfigErrorKind};
///
/// let err = Config::from_toml_str("this is not valid = = toml").unwrap_err();
/// assert_eq!(err.kind(), ConfigErrorKind::Parse);
/// ```
#[non_exhaustive]
pub struct ConfigError(ConfigErrorRepr);

/// The classification of a [`ConfigError`], for stable caller decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigErrorKind {
    /// The configuration file could not be read.
    Read,
    /// The configuration was not valid TOML.
    Parse,
    /// A `${...}` interpolation was malformed.
    Interpolation,
    /// A `${VAR}` referenced an unset environment variable.
    UnresolvedVar,
    /// The configuration parsed but failed a semantic check.
    Validation,
    /// An `include` chain revisited a file already being resolved.
    IncludeCycle,
    /// An `include` chain exceeded the maximum nesting depth.
    IncludeDepth,
}

impl ConfigError {
    /// Classify this failure without matching a private representation.
    #[must_use]
    pub fn kind(&self) -> ConfigErrorKind {
        match self.0 {
            ConfigErrorRepr::Read { .. } => ConfigErrorKind::Read,
            ConfigErrorRepr::Parse(_) => ConfigErrorKind::Parse,
            ConfigErrorRepr::Interpolation(_) => ConfigErrorKind::Interpolation,
            ConfigErrorRepr::UnresolvedVar(_) => ConfigErrorKind::UnresolvedVar,
            ConfigErrorRepr::Validation(_) => ConfigErrorKind::Validation,
            ConfigErrorRepr::IncludeCycle { .. } => ConfigErrorKind::IncludeCycle,
            ConfigErrorRepr::IncludeDepth { .. } => ConfigErrorKind::IncludeDepth,
        }
    }
}

impl std::fmt::Debug for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.0, f)
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<ConfigErrorRepr> for ConfigError {
    fn from(repr: ConfigErrorRepr) -> Self {
        ConfigError(repr)
    }
}

/// A startup failure while assembling or serving the gateway.
///
/// Opaque wrapper preserving the underlying cause via `source()`. Classify with
/// [`StartupError::kind`].
///
/// # Examples
/// ```no_run
/// use promptforge_gateway::{run, ConfigSource, ProfileName, ServeOptions, StartupErrorKind};
/// use std::path::PathBuf;
///
/// # fn demo() {
/// let options = ServeOptions::new(
///     PathBuf::from("/tmp/profiles"),
///     ConfigSource::Profile(ProfileName::parse("dev").unwrap()),
/// );
/// if let Err(err) = run(options) {
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
}

#[derive(Debug, thiserror::Error)]
enum StartupRepr {
    #[error("configuration error")]
    Config(#[source] ConfigErrorRepr),
    #[error("local provisioning error")]
    Provisioning(#[source] LocalError),
    #[error("failed to bind the listener")]
    Bind(#[source] std::io::Error),
    #[error("serve error")]
    Serve(#[source] ServeReprSource),
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
        }
    }

    pub(crate) fn config(err: ConfigErrorRepr) -> Self {
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
