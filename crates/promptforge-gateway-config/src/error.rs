//! The private representation behind the public [`crate::ConfigError`].

/// A configuration load or validation failure.
///
/// Paths are kept as [`PathBuf`](std::path::PathBuf) and the include chain as a
/// `Vec<PathBuf>` (ERR-006); the TOML parse cause is preserved as a private
/// `#[source]` rather than flattened into a string (ERR-002).
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

    /// An `include` chain revisited a file already being resolved.
    #[non_exhaustive]
    #[error("include cycle at {} (chain: {})", path.display(), render_chain(chain))]
    IncludeCycle {
        /// The path that closed the cycle.
        path: std::path::PathBuf,
        /// The include stack when the cycle was detected.
        chain: Vec<std::path::PathBuf>,
    },

    /// An `include` chain exceeded the maximum nesting depth.
    #[non_exhaustive]
    #[error("include depth exceeded {max} at {}", path.display())]
    IncludeDepth {
        /// The path that would have been loaded next.
        path: std::path::PathBuf,
        /// The configured maximum depth.
        max: usize,
    },
}

/// Renders the optional parse-failure path as a ` (path)` suffix or empty.
fn parse_location(path: Option<&std::path::PathBuf>) -> String {
    path.map(|p| format!(" ({})", p.display()))
        .unwrap_or_default()
}

/// Renders an include chain as `a -> b -> c`.
fn render_chain(chain: &[std::path::PathBuf]) -> String {
    chain
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" -> ")
}
