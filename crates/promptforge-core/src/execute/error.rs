//! The public run-error surface: [`RunError`] and its stable [`RunErrorKind`].

use std::fmt;

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
    /// A section's Lua phase failed to run or return a usable value.
    Lua,
    /// A Lua host resource quota (log events, log bytes, or instructions) was
    /// exhausted.
    Quota,
    /// A `{{ }}` prose substitution failed.
    Substitution,
    /// The host cancelled the run.
    Cancelled,
    /// An unexpected internal invariant failure.
    Internal,
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
            Error::LuaCompile { .. } | Error::Lua(_) | Error::LuaRuntime { .. } => {
                RunErrorKind::Lua
            }
            Error::UnsupportedVersion(_) => RunErrorKind::Version,
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
            | Error::EmptyModelReply { .. }
            | Error::DialectNone
            | Error::DialectTie { .. }
            | Error::UnknownDialect(_) => RunErrorKind::Completion,
            Error::Interrupted => RunErrorKind::Cancelled,
            Error::Substitution(_) => RunErrorKind::Substitution,
            Error::ToolLoopExhausted | Error::OutOfScopeToolCall { .. } | Error::Tool { .. } => {
                RunErrorKind::Tool
            }
            Error::FanoutArmJoin(_) | Error::Internal(_) | Error::TimestampFormat(_) => {
                RunErrorKind::Internal
            }
            Error::Bind { .. }
            | Error::BindSchema { .. }
            | Error::BindQuery { .. }
            | Error::Absent { .. }
            | Error::Duplicate { .. }
            | Error::Ambiguous { .. }
            | Error::DuplicateAlias { .. }
            | Error::ToolIdSelectedTwice { .. }
            | Error::PickedToolNotLive { .. }
            | Error::ToolScopeAnalysisSource { .. }
            | Error::NearDuplicateTools { .. }
            | Error::ModelBind { .. }
            | Error::ModelBindQuery { .. }
            | Error::ModelAbsent { .. }
            | Error::ModelDuplicate { .. }
            | Error::ModelAmbiguous { .. }
            | Error::DuplicateModelAlias { .. }
            | Error::ModelRequired { .. } => RunErrorKind::Binding,
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
