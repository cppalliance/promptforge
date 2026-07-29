//! PromptForge runtime core.
//!
//! This crate holds the pieces that turn a prompt markdown file into a model
//! call: the [`parser`] that reads the file into a [`parser::Prompt`], the
//! [`client`] that talks to an `OpenAI`-compatible chat completions endpoint, and
//! [`execute`] that runs a prompt's first section and returns the model's text.
//!
//! Tranche 1 scope: parse, send the entry section's prose, print the reply. No
//! tools, no Lua, no multi-section fall-through yet.

pub mod client;
pub mod execute;
pub mod lua;
pub mod parser;
pub mod subst;
pub mod tools;

/// The crate's error type, spanning parsing, HTTP, and execution failures.
///
/// Marked `#[non_exhaustive]` so future variants are not a breaking change, and
/// the transport variant hides its concrete source type so no dependency's
/// error leaks through this crate's public API.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The prompt file could not be parsed (bad frontmatter, no sections, etc.).
    #[error("parse error: {0}")]
    Parse(String),

    /// A required environment variable was missing.
    #[error("missing environment variable: {0}")]
    MissingEnv(String),

    /// The HTTP request to the model backend failed at the transport layer.
    #[error("http transport error")]
    Http(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The backend returned a non-success status.
    #[error("backend returned {status}: {body}")]
    Backend {
        /// The HTTP status code returned by the backend.
        status: u16,
        /// The (truncated) response body, for diagnostics.
        body: String,
    },

    /// The backend response could not be understood (missing choices, etc.).
    #[error("malformed response: {0}")]
    MalformedResponse(String),

    /// A section's Lua block failed to build, run, or return a usable value.
    #[error("lua error: {0}")]
    Lua(String),

    /// A `{{ }}` prose substitution failed (unknown/missing path, unclosed).
    #[error("substitution error: {0}")]
    Substitution(String),

    /// The tool-call loop ran its iteration cap without a final text reply.
    #[error("tool-call loop did not converge")]
    ToolLoopExhausted,

    /// The model asked to call a tool that was not provided to the executor.
    #[error("model called unknown tool {0}")]
    UnknownTool(String),

    /// A section's Lua block scoped a tool by a name absent from the run's
    /// frontmatter tools, so no matching tool could be advertised or dispatched.
    #[error("section scoped unknown tool {0}")]
    UnknownScopedTool(String),
}

impl Error {
    /// Wrap a transport-layer error, hiding its concrete type from the API.
    pub(crate) fn http(source: reqwest::Error) -> Error {
        Error::Http(Box::new(source))
    }
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;
