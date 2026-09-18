//! The public run-error surface: [`RunError`] and its stable [`RunErrorKind`].

use std::fmt;
use std::ops::Range;

use crate::Error;

/// A stable, matchable classification of a [`RunError`].
///
/// The variant identifies the phase of the run that failed without exposing the
/// internal error substrate. It is `#[non_exhaustive]`, so new kinds can be
/// added without breaking a caller's `match`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunErrorKind {
    /// The prompt could not be parsed or a compiled Lua region was invalid.
    Parse,
    /// The prompt declared a `promptforge:` major this build does not support.
    Version,
    /// A tool or model capability could not be bound, was absent, or clashed.
    Binding,
    /// A model completion failed at the transport, backend, or decode layer.
    Completion,
    /// A dispatched tool failed, was unknown, or the tool loop did not converge.
    Tool,
    /// A run-scoped store operation failed.
    Store,
    /// Two live execution identities claimed one store path: the claims
    /// model terminated the run to keep interleaving deterministic.
    Determinism,
    /// A section's Lua phase failed to run or return a usable value.
    Lua,
    /// A Lua host resource quota (log events, log bytes, or instructions) was
    /// exhausted.
    Quota,
    /// The selected compactor exhausted the model's context window.
    ContextExhausted,
    /// The host's input broker failed a `user_input` request.
    Input,
    /// A `{{ }}` prose substitution failed.
    Substitution,
    /// The host cancelled the run.
    Cancelled,
    /// An unexpected internal invariant failure.
    Internal,
    /// An H1 assertion or model requirement the environment cannot satisfy.
    RequirementsUnmet,
}

/// Where a failure lives: a prompt source position or a Rust code position.
///
/// One generic shape - the [`RunErrorKind`] says which world the fault is in,
/// and the path's extension says it again. Kinds are for code, messages for
/// reading, locations for navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    /// The prompt's frontmatter name when parse got that far, or the Rust
    /// source file (from `file!()`) for an internal fault. A frontmatter
    /// YAML failure predates the name, so its path is a placeholder the
    /// host replaces with its own label for the source.
    pub path: String,
    /// The 1-based line, when known.
    pub line: Option<u32>,
    /// The 1-based column, when known.
    pub column: Option<u32>,
    /// The byte span of the offending region, as today, when known.
    pub span: Option<Range<usize>>,
}

/// The error returned by [`run`](super::run), the orchestration boundary of a
/// prompt run.
///
/// A `RunError` carries a stable [`kind`](RunError::kind) classifier plus the
/// `is_cancelled`/`is_retryable` predicates, and preserves the underlying cause
/// through [`std::error::Error::source`]. It is `#[non_exhaustive]` and cannot
/// be constructed outside the crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct RunError {
    inner: Error,
}

impl RunError {
    /// Returns the stable classification of this failure.
    #[must_use]
    pub fn kind(&self) -> RunErrorKind {
        match &self.inner {
            Error::ParseStructured { .. } | Error::ParseFrontmatter { .. } => RunErrorKind::Parse,
            Error::LuaQuota { .. } => RunErrorKind::Quota,
            Error::ContextExhausted { .. } => RunErrorKind::ContextExhausted,
            Error::Input { .. } => RunErrorKind::Input,
            Error::LuaCompile { .. } | Error::Lua(_) | Error::LuaRuntime { .. } => {
                RunErrorKind::Lua
            }
            Error::UnsupportedVersion(_) => RunErrorKind::Version,
            Error::RequirementsUnmet { .. } => RunErrorKind::RequirementsUnmet,
            Error::MissingEnv(_)
            | Error::InvalidEnv(_)
            | Error::InvalidConfig(_)
            | Error::Config { .. }
            | Error::GatewayDisabled
            | Error::Http(_)
            | Error::Backend { .. }
            | Error::BackendBodyRead { .. }
            | Error::MalformedResponse(_)
            | Error::MalformedResponseSource { .. }
            | Error::EmptyModelReply { .. } => RunErrorKind::Completion,
            Error::Interrupted => RunErrorKind::Cancelled,
            Error::Substitution(_) => RunErrorKind::Substitution,
            Error::ToolLoopExhausted
            | Error::OutOfScopeToolCall { .. }
            | Error::UnboundToolCall { .. }
            | Error::Tool { .. } => RunErrorKind::Tool,
            Error::Internal { .. } | Error::TimestampFormat(_) => RunErrorKind::Internal,
            Error::Store(_) => RunErrorKind::Store,
            Error::Determinism(_) => RunErrorKind::Determinism,
            Error::BindSchema { .. } | Error::ModelRequired { .. } => RunErrorKind::Binding,
        }
    }

    /// Returns `true` when the run failed because the host cancelled it.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self.inner, Error::Interrupted)
    }

    /// Returns `true` when retrying the run may succeed (transient transport or
    /// backend failures).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match &self.inner {
            Error::Http(_)
            | Error::MalformedResponse(_)
            | Error::MalformedResponseSource { .. }
            | Error::BackendBodyRead { .. } => true,
            Error::Backend { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Returns where the failure lives, when it has a location.
    ///
    /// Parse-kind failures carry the prompt source position (the frontmatter
    /// name as the path when parse got that far, plus the surfaced YAML
    /// line/column or the span-derived position); internal faults carry the
    /// Rust source file and line of the broken invariant. Other kinds have
    /// no source position to navigate to and return `None`.
    #[must_use]
    pub fn location(&self) -> Option<SourceLocation> {
        match &self.inner {
            Error::ParseFrontmatter { line, column, .. } => Some(SourceLocation {
                path: "<prompt>".to_owned(),
                line: *line,
                column: *column,
                span: None,
            }),
            Error::ParseStructured {
                name,
                line,
                column,
                span,
                ..
            } => Some(SourceLocation {
                path: name.clone().unwrap_or_else(|| "<prompt>".to_owned()),
                line: *line,
                column: *column,
                span: span.map(|(start, end)| start..end),
            }),
            Error::Internal { file, line, .. } => Some(SourceLocation {
                path: (*file).to_owned(),
                line: Some(*line),
                column: None,
                span: None,
            }),
            _ => None,
        }
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.inner)
    }
}

impl From<Error> for RunError {
    fn from(inner: Error) -> Self {
        RunError { inner }
    }
}

impl From<RunError> for Error {
    fn from(error: RunError) -> Self {
        error.inner
    }
}
