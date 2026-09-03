//! Opaque, source-preserving public error type for configuration loading.
//!
//! The public [`ConfigError`] is a thin wrapper over a private representation.
//! The representation carries the underlying cause via
//! [`std::error::Error::source`] and never appears in a public signature, so no
//! dependency type (toml, io) leaks into the crate's semver surface. Callers
//! classify failures with the `kind()` method instead of matching private
//! variants.

use crate::error::ConfigError as ConfigErrorRepr;

/// A configuration load or validation failure.
///
/// Opaque wrapper: the underlying `toml`, `io`, and validation detail are kept
/// as a private `source()`; classify with [`ConfigError::kind`].
///
/// # Examples
/// ```
/// use gateway_config::{Config, ConfigErrorKind};
///
/// let err =
///     Config::from_toml_str("config-version = 2\nthis is not valid = = toml").unwrap_err();
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
    /// A removed configuration layout feature was found.
    HardBreak,
    /// A shadow file could not be written.
    Write,
}

impl ConfigError {
    /// Classify this failure without matching a private representation.
    #[must_use]
    pub fn kind(&self) -> ConfigErrorKind {
        match self.0 {
            ConfigErrorRepr::Read { .. } => ConfigErrorKind::Read,
            ConfigErrorRepr::Parse { .. } => ConfigErrorKind::Parse,
            ConfigErrorRepr::Interpolation(_) => ConfigErrorKind::Interpolation,
            ConfigErrorRepr::UnresolvedVar(_) => ConfigErrorKind::UnresolvedVar,
            ConfigErrorRepr::Validation(_) => ConfigErrorKind::Validation,
            ConfigErrorRepr::HardBreak { .. } => ConfigErrorKind::HardBreak,
            ConfigErrorRepr::Write { .. } => ConfigErrorKind::Write,
        }
    }

    /// A semantic validation failure carrying `message`.
    ///
    /// For consumers that build on a validated [`Config`](crate::Config) and
    /// need to report a referential-integrity failure of their own (for
    /// example the gateway's routing table construction) in the same error
    /// channel.
    #[must_use]
    pub fn validation(message: String) -> ConfigError {
        ConfigError(ConfigErrorRepr::Validation(message))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;
    use std::path::PathBuf;

    fn de_error() -> toml::de::Error {
        toml::from_str::<toml::Table>("= bad").unwrap_err()
    }

    #[test]
    fn config_error_kind_is_table_driven() {
        let cases: Vec<(ConfigErrorRepr, ConfigErrorKind)> = vec![
            (
                ConfigErrorRepr::Read {
                    path: PathBuf::from("c.toml"),
                    source: std::io::Error::other("x"),
                },
                ConfigErrorKind::Read,
            ),
            (
                ConfigErrorRepr::Parse {
                    path: None,
                    source: Box::new(de_error()),
                },
                ConfigErrorKind::Parse,
            ),
            (
                ConfigErrorRepr::UnresolvedVar("V".to_owned()),
                ConfigErrorKind::UnresolvedVar,
            ),
            (
                ConfigErrorRepr::Interpolation("bad".to_owned()),
                ConfigErrorKind::Interpolation,
            ),
            (
                ConfigErrorRepr::Validation("bad".to_owned()),
                ConfigErrorKind::Validation,
            ),
            (
                ConfigErrorRepr::HardBreak {
                    path: PathBuf::from("a.toml"),
                    line: 4,
                    key: "include",
                    replacement: "use one gateway.toml with [[profile]] entries",
                },
                ConfigErrorKind::HardBreak,
            ),
            (
                ConfigErrorRepr::Write {
                    path: PathBuf::from("a.toml.next"),
                    source: std::io::Error::other("x"),
                },
                ConfigErrorKind::Write,
            ),
        ];
        for (repr, kind) in cases {
            assert_eq!(ConfigError::from(repr).kind(), kind);
        }
    }

    #[test]
    fn config_error_read_and_parse_preserve_source_and_path() {
        let read = ConfigError::from(ConfigErrorRepr::Read {
            path: PathBuf::from("c.toml"),
            source: std::io::Error::other("x"),
        });
        assert!(read.source().is_some());
        assert!(read.to_string().contains("c.toml"));

        let parse = ConfigError::from(ConfigErrorRepr::Parse {
            path: Some(PathBuf::from("inc.toml")),
            source: Box::new(de_error()),
        });
        assert!(parse.source().is_some());
        assert!(parse.to_string().contains("inc.toml"));
    }

    #[test]
    fn validation_constructor_classifies_as_validation() {
        let err = ConfigError::validation("bad".to_owned());
        assert_eq!(err.kind(), ConfigErrorKind::Validation);
        assert!(err.to_string().contains("bad"));
    }
}
