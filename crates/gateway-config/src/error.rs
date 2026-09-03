//! The private representation behind the public [`crate::ConfigError`].

/// A configuration load or validation failure.
///
/// Paths stay as [`PathBuf`](std::path::PathBuf), and TOML parse causes remain
/// private `#[source]` values rather than flattened strings.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum ConfigError {
    /// The configuration file could not be read.
    #[non_exhaustive]
    #[error("read config {}", path.display())]
    Read {
        /// The path that could not be read.
        path: std::path::PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration was not valid TOML.
    #[non_exhaustive]
    #[error("parse config{}", parse_location(path.as_ref()))]
    Parse {
        /// The file the parse failure came from, when known.
        path: Option<std::path::PathBuf>,
        /// The underlying TOML deserialization error (boxed: it is large).
        #[source]
        source: Box<toml::de::Error>,
    },

    /// A `${VAR}` referenced an environment variable that was not set.
    #[non_exhaustive]
    #[error("unresolved environment variable {0}")]
    UnresolvedVar(String),

    /// A `${...}` interpolation was malformed (for example, unclosed).
    #[non_exhaustive]
    #[error("interpolation: {0}")]
    Interpolation(String),

    /// The configuration parsed but failed a semantic check.
    #[non_exhaustive]
    #[error("invalid config: {0}")]
    Validation(String),

    /// A removed layout feature was found with an actionable source location.
    #[non_exhaustive]
    #[error(
        "{}:{line}: removed config key `{key}`; {replacement}",
        path.display()
    )]
    HardBreak {
        /// The file containing the removed layout.
        path: std::path::PathBuf,
        /// One-based source line.
        line: usize,
        /// Removed key or layout feature.
        key: &'static str,
        /// One-sentence replacement.
        replacement: &'static str,
    },

    /// A shadow file could not be written.
    #[non_exhaustive]
    #[error("write shadow {}", path.display())]
    Write {
        /// The path that could not be written.
        path: std::path::PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Renders the optional parse-failure path as a ` (path)` suffix or empty.
fn parse_location(path: Option<&std::path::PathBuf>) -> String {
    path.map(|p| format!(" ({})", p.display()))
        .unwrap_or_default()
}
