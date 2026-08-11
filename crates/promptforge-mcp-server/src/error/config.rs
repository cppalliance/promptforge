//! The `prompts.toml` load failure and its classification.

use std::fmt;
use std::path::PathBuf;

/// A `prompts.toml` load failure.
///
/// Opaque: the representation is private, so a caller classifies with
/// [`ConfigError::kind`], reads the failing path (when one is known) through
/// [`ConfigError::path`], and reads the underlying cause through
/// [`std::error::Error::source`], rather than matching a variant or reading a
/// public field. No `String` payload or dependency error type reaches this
/// crate's public surface.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use promptforge_mcp_server::{Config, ConfigErrorKind};
///
/// // A path that cannot be read classifies as `Read`, names itself, and
/// // carries the underlying `io::Error` as its source.
/// let missing = Path::new("no-such-config-\u{1}.toml");
/// let err = Config::load(missing).unwrap_err();
/// assert_eq!(err.kind(), ConfigErrorKind::Read);
/// assert_eq!(err.path(), Some(missing));
/// assert!(std::error::Error::source(&err).is_some());
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct ConfigError {
    repr: ConfigErrorRepr,
}

/// The private representation of a [`ConfigError`]. Kept out of the public
/// surface so no `String` payload or dependency error type is exposed and the
/// shape stays free to change behind [`ConfigError::kind`].
#[derive(Debug)]
enum ConfigErrorRepr {
    /// The configuration file could not be read. Carries the path losslessly
    /// and the underlying I/O error as its source.
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The configuration was not valid TOML, or a value had the wrong shape.
    /// The rendered message is private; the underlying parser error, when there
    /// is one, is kept as the source.
    Parse {
        message: String,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// `[server].token` was present but carried nothing.
    EmptyToken,
    /// A `${VAR}` referenced an environment variable that was not set.
    UnresolvedVar { name: String },
    /// A `${...}` interpolation was malformed (for example, unclosed).
    Interpolation { detail: String },
}

impl ConfigError {
    /// The configuration file could not be read.
    pub(crate) fn read(path: PathBuf, source: std::io::Error) -> ConfigError {
        ConfigError {
            repr: ConfigErrorRepr::Read { path, source },
        }
    }

    /// A value had the wrong shape or failed a boundary check, described by
    /// `message`, with no separate underlying error to preserve.
    pub(crate) fn parse(message: impl Into<String>) -> ConfigError {
        ConfigError {
            repr: ConfigErrorRepr::Parse {
                message: message.into(),
                source: None,
            },
        }
    }

    /// The TOML would not parse or deserialize. The parser error is rendered
    /// for `Display` and kept as the source for the cause chain.
    pub(crate) fn parse_toml(source: toml::de::Error) -> ConfigError {
        ConfigError {
            repr: ConfigErrorRepr::Parse {
                message: source.to_string(),
                source: Some(Box::new(source)),
            },
        }
    }

    /// `[server].token` was present but carried nothing.
    pub(crate) fn empty_token() -> ConfigError {
        ConfigError {
            repr: ConfigErrorRepr::EmptyToken,
        }
    }

    /// A `${VAR}` referenced an environment variable that was not set.
    pub(crate) fn unresolved_var(name: impl Into<String>) -> ConfigError {
        ConfigError {
            repr: ConfigErrorRepr::UnresolvedVar { name: name.into() },
        }
    }

    /// A `${...}` interpolation was malformed.
    pub(crate) fn interpolation(detail: impl Into<String>) -> ConfigError {
        ConfigError {
            repr: ConfigErrorRepr::Interpolation {
                detail: detail.into(),
            },
        }
    }

    /// The path a read failure names, losslessly, when the failure is about a
    /// file. `None` for a failure that is not about a specific path.
    #[must_use]
    pub fn path(&self) -> Option<&std::path::Path> {
        match &self.repr {
            ConfigErrorRepr::Read { path, .. } => Some(path.as_path()),
            _ => None,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            ConfigErrorRepr::Read { path, .. } => write!(f, "read config {}", path.display()),
            ConfigErrorRepr::Parse { message, .. } => write!(f, "parse config: {message}"),
            ConfigErrorRepr::EmptyToken => f.write_str("[server].token must not be empty"),
            ConfigErrorRepr::UnresolvedVar { name } => {
                write!(f, "unresolved environment variable {name}")
            }
            ConfigErrorRepr::Interpolation { detail } => write!(f, "interpolation: {detail}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.repr {
            ConfigErrorRepr::Read { source, .. } => Some(source),
            ConfigErrorRepr::Parse { source, .. } => source
                .as_deref()
                .map(|s| s as &(dyn std::error::Error + 'static)),
            _ => None,
        }
    }
}

/// A stable, dependency-free classification of a [`ConfigError`].
///
/// Callers switch on `kind()` rather than matching the error's variants
/// directly, so the private representation stays free to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConfigErrorKind {
    /// The configuration file could not be read.
    Read,
    /// The TOML was invalid, or a value had the wrong shape.
    Parse,
    /// `[server].token` was present but carried nothing.
    EmptyToken,
    /// A `${VAR}` referenced an environment variable that was not set.
    UnresolvedVar,
    /// A `${...}` interpolation was malformed.
    Interpolation,
}

impl ConfigError {
    /// Classifies the failure without exposing the error's representation.
    #[must_use]
    pub fn kind(&self) -> ConfigErrorKind {
        match &self.repr {
            ConfigErrorRepr::Read { .. } => ConfigErrorKind::Read,
            ConfigErrorRepr::Parse { .. } => ConfigErrorKind::Parse,
            ConfigErrorRepr::EmptyToken => ConfigErrorKind::EmptyToken,
            ConfigErrorRepr::UnresolvedVar { .. } => ConfigErrorKind::UnresolvedVar,
            ConfigErrorRepr::Interpolation { .. } => ConfigErrorKind::Interpolation,
        }
    }
}
