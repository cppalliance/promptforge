//! The boot failure that [`run`](crate::run) returns, one variant per ordered
//! boot step, each carrying that step's own opaque error as its source.

use std::fmt;

use super::{CatalogError, ConfigError, PreparedToolsError, ServeError, WatchError};

/// Booting the server failed at one of its ordered steps.
///
/// The steps run in sequence - load the configuration, resolve the catalog,
/// prepare the tool environment, start the watcher, serve - and this is the
/// first one that could not proceed. It is what
/// [`run`](crate::run) returns so the binary stays a thin shell that only
/// renders the failure.
///
/// Opaque: the representation is private, so a caller classifies with
/// [`RunError::kind`] and reads the underlying cause through
/// [`std::error::Error::source`], rather than matching a variant. No dependency
/// error type reaches this crate's public surface; each step's own opaque error
/// is the source.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use promptforge_mcp_server::{Config, RunError, RunErrorKind};
///
/// // A boot that fails at its configuration step surfaces as a `RunError`
/// // classified `Config`, carrying the underlying `ConfigError` as its source.
/// let config_error = Config::load(Path::new("no-such-config-\u{1}.toml")).unwrap_err();
/// let err = RunError::from(config_error);
/// assert_eq!(err.kind(), RunErrorKind::Config);
/// assert!(std::error::Error::source(&err).is_some());
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct RunError {
    repr: RunErrorRepr,
}

/// The private representation of a [`RunError`]. Each boot step contributes one
/// variant carrying that step's own error as the source.
#[derive(Debug)]
enum RunErrorRepr {
    /// The configuration file could not be loaded.
    Config(ConfigError),
    /// The prompt catalog could not be resolved.
    Catalog(CatalogError),
    /// The immutable tool environment could not be prepared.
    Prepare(PreparedToolsError),
    /// The prompt watcher could not be started.
    Watch(WatchError),
    /// The chosen transport would not start, or stopped abnormally.
    Serve(ServeError),
    /// The async runtime could not be built.
    Runtime(std::io::Error),
}

impl From<ConfigError> for RunError {
    fn from(source: ConfigError) -> RunError {
        RunError {
            repr: RunErrorRepr::Config(source),
        }
    }
}

impl From<CatalogError> for RunError {
    fn from(source: CatalogError) -> RunError {
        RunError {
            repr: RunErrorRepr::Catalog(source),
        }
    }
}

impl From<PreparedToolsError> for RunError {
    fn from(source: PreparedToolsError) -> RunError {
        RunError {
            repr: RunErrorRepr::Prepare(source),
        }
    }
}

impl From<WatchError> for RunError {
    fn from(source: WatchError) -> RunError {
        RunError {
            repr: RunErrorRepr::Watch(source),
        }
    }
}

impl From<ServeError> for RunError {
    fn from(source: ServeError) -> RunError {
        RunError {
            repr: RunErrorRepr::Serve(source),
        }
    }
}

impl RunError {
    /// The async runtime could not be built.
    pub(crate) fn runtime(source: std::io::Error) -> RunError {
        RunError {
            repr: RunErrorRepr::Runtime(source),
        }
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            RunErrorRepr::Config(_) => f.write_str("load the configuration"),
            RunErrorRepr::Catalog(_) => f.write_str("resolve the prompt catalog"),
            RunErrorRepr::Prepare(_) => f.write_str("prepare the tool environment"),
            RunErrorRepr::Watch(_) => f.write_str("start the prompt watcher"),
            RunErrorRepr::Serve(_) => f.write_str("serve the transport"),
            RunErrorRepr::Runtime(_) => f.write_str("build the async runtime"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.repr {
            RunErrorRepr::Config(source) => Some(source),
            RunErrorRepr::Catalog(source) => Some(source),
            RunErrorRepr::Prepare(source) => Some(source),
            RunErrorRepr::Watch(source) => Some(source),
            RunErrorRepr::Serve(source) => Some(source),
            RunErrorRepr::Runtime(source) => Some(source),
        }
    }
}

/// A stable, dependency-free classification of a [`RunError`].
///
/// # Examples
/// ```
/// use promptforge_mcp_server::RunErrorKind;
///
/// // The classification is a plain `Copy` value a caller can match or store
/// // without touching the error's representation.
/// fn is_runtime_failure(kind: RunErrorKind) -> bool {
///     matches!(kind, RunErrorKind::Runtime)
/// }
/// assert!(is_runtime_failure(RunErrorKind::Runtime));
/// assert_ne!(RunErrorKind::Config, RunErrorKind::Serve);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RunErrorKind {
    /// The configuration file could not be loaded.
    Config,
    /// The prompt catalog could not be resolved.
    Catalog,
    /// The immutable tool environment could not be prepared.
    Prepare,
    /// The prompt watcher could not be started.
    Watch,
    /// The chosen transport would not start, or stopped abnormally.
    Serve,
    /// The async runtime could not be built.
    Runtime,
}

impl RunError {
    /// Classifies the failure without exposing the error's representation.
    ///
    /// # Examples
    /// ```
    /// use std::path::Path;
    /// use promptforge_mcp_server::{Config, RunError, RunErrorKind};
    ///
    /// let err = RunError::from(Config::load(Path::new("no-such-config-\u{1}.toml")).unwrap_err());
    /// assert_eq!(err.kind(), RunErrorKind::Config);
    /// ```
    #[must_use]
    pub fn kind(&self) -> RunErrorKind {
        match &self.repr {
            RunErrorRepr::Config(_) => RunErrorKind::Config,
            RunErrorRepr::Catalog(_) => RunErrorKind::Catalog,
            RunErrorRepr::Prepare(_) => RunErrorKind::Prepare,
            RunErrorRepr::Watch(_) => RunErrorKind::Watch,
            RunErrorRepr::Serve(_) => RunErrorKind::Serve,
            RunErrorRepr::Runtime(_) => RunErrorKind::Runtime,
        }
    }
}
