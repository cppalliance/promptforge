//! Error types for the MCP server.

/// A `prompts.toml` load failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[non_exhaustive]
    #[error("read config {path}")]
    Read {
        /// The path that could not be read.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration was not valid TOML, or a value had the wrong shape.
    #[non_exhaustive]
    #[error("parse config: {0}")]
    Parse(String),

    /// A `${VAR}` referenced an environment variable that was not set.
    #[non_exhaustive]
    #[error("unresolved environment variable {0}")]
    UnresolvedVar(String),

    /// A `${...}` interpolation was malformed (for example, unclosed).
    #[non_exhaustive]
    #[error("interpolation: {0}")]
    Interpolation(String),
}
