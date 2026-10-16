//! The public run-error surface: [`RunError`] and its stable [`RunErrorKind`].

use std::fmt;
use std::ops::Range;

use crate::Error;

/// The stable category of a [`RunError`], for matching in code.
///
/// Each variant names the phase of the run that failed. The enum is
/// `#[non_exhaustive]`, so a `match` on it needs a wildcard arm. That lets
/// new kinds be added while existing callers keep compiling.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunErrorKind {
    /// Parsing the prompt failed, or a compiled Lua region was invalid.
    Parse,
    /// The prompt declared a `runvil:` major version outside the set
    /// this build supports.
    Version,
    /// Binding a tool or model capability failed, or the capability was
    /// absent.
    Binding,
    /// A model completion failed at the transport, backend, or decode layer.
    Completion,
    /// A tool call failed or named an alias outside its scope, or the
    /// tool-call loop reached its iteration cap before a final reply.
    Tool,
    /// A run-scoped store operation failed, or the run lacks a store to
    /// operate on.
    Vfs,
    /// Two live executions in the run claimed the same store path.
    ///
    /// The run stops at once to keep the interleaving of their store access
    /// deterministic.
    Determinism,
    /// A section's Lua phase failed to run or return a usable value.
    Lua,
    /// A Lua resource quota (log events, log bytes, or instructions) was
    /// exhausted, or a `call`, `tasks.spawn`, or `tasks.fanout` arm would
    /// have nested past the call-depth cap.
    Quota,
    /// The request overflowed the model's context window, and the selected
    /// compactor left it over the limit.
    ContextExhausted,
    /// A `{{ }}` prose substitution failed.
    Substitution,
    /// The caller cancelled the run.
    Cancelled,
    /// An internal invariant failed.
    Internal,
    /// The environment falls short of a requirement the prompt states, such
    /// as an H1 assertion or a model requirement.
    RequirementsUnmet,
}

/// Where a run failed: a position in the prompt source or in the Rust
/// source.
///
/// The error's [`RunErrorKind`] says which of the two it is, and the
/// extension of `path` says so too. Use the kind to branch in code, the
/// error message to show a reader, and the location to navigate to the
/// fault. Build one with [`new`](SourceLocation::new) and the `with_`
/// methods.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SourceLocation {
    /// The prompt name or Rust source file where the failure occurred.
    ///
    /// For a prompt failure this is the prompt's frontmatter name when
    /// parsing got that far. Otherwise it is the placeholder `<prompt>`,
    /// which the caller should replace with its own label for the source. A
    /// frontmatter YAML failure always gets the placeholder, because it
    /// happens before the name is read. For an internal fault this is the
    /// Rust source file, as given by `file!()`.
    pub path: String,
    /// The 1-based line, when known.
    pub line: Option<u32>,
    /// The 1-based column, when known.
    pub column: Option<u32>,
    /// The byte span of the offending region, when known.
    ///
    /// Only parse failures can have a span, and a frontmatter YAML failure
    /// always has `None`. The offsets index the document body, which
    /// excludes the frontmatter and any leading BOM and has CRLF line endings
    /// normalized to LF. Use [`line`](SourceLocation::line) and
    /// [`column`](SourceLocation::column) to locate the failure in the
    /// original file.
    pub span: Option<Range<usize>>,
}

impl SourceLocation {
    /// Builds a location in `path` with no line, column, or span.
    #[must_use]
    pub fn new(path: impl Into<String>) -> SourceLocation {
        SourceLocation {
            path: path.into(),
            line: None,
            column: None,
            span: None,
        }
    }

    /// Sets the 1-based line.
    #[must_use]
    pub fn with_line(mut self, line: Option<u32>) -> SourceLocation {
        self.line = line;
        self
    }

    /// Sets the 1-based column.
    #[must_use]
    pub fn with_column(mut self, column: Option<u32>) -> SourceLocation {
        self.column = column;
        self
    }

    /// Sets the byte span of the offending region in the document body.
    #[must_use]
    pub fn with_span(mut self, span: Option<Range<usize>>) -> SourceLocation {
        self.span = span;
        self
    }
}

/// The error a failed prompt run reports.
///
/// A run that fails ends with [`Step::Done`](super::Step::Done), whose
/// result is [`RunResult::Failure`](super::RunResult::Failure) holding this
/// error. Use [`kind`](RunError::kind) to classify the failure in code. The
/// message is self-contained: when the failure keeps an originating error,
/// the message already holds that error's text, and
/// [`cause`](RunError::cause) returns the error itself. A failure whose
/// message does not hold its underlying error, such as a model round's,
/// keeps it behind [`std::error::Error::source`] instead.
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
            Error::LuaQuota { .. } | Error::CallDepthExceeded { .. } => RunErrorKind::Quota,
            Error::ContextExhausted { .. } => RunErrorKind::ContextExhausted,
            // A leaked task, a task reached for by a chain that does not
            // own it, a result waited on twice, and a wait's delivery of a
            // cancelled task surfacing uncaught are the author's program
            // failing, as any Lua fault is.
            Error::LuaCompile { .. }
            | Error::Lua(_)
            | Error::LuaRuntime { .. }
            | Error::TasksLive { .. }
            | Error::TaskNotOwned { .. }
            | Error::TaskConsumed { .. }
            | Error::TaskCancelled { .. } => RunErrorKind::Lua,
            Error::UnsupportedVersion(_) => RunErrorKind::Version,
            Error::RequirementsUnmet { .. } => RunErrorKind::RequirementsUnmet,
            Error::Completion(_) => RunErrorKind::Completion,
            Error::Interrupted => RunErrorKind::Cancelled,
            Error::Substitution(_) => RunErrorKind::Substitution,
            Error::ToolLoopExhausted
            | Error::OutOfScopeToolCall { .. }
            | Error::UnboundToolCall { .. }
            | Error::Tool { .. } => RunErrorKind::Tool,
            Error::Internal { .. } => RunErrorKind::Internal,
            Error::Store { .. } => RunErrorKind::Vfs,
            Error::Determinism(_) => RunErrorKind::Determinism,
            Error::BindSchema { .. } | Error::ModelRequired { .. } => RunErrorKind::Binding,
        }
    }

    /// Returns `true` when the run failed because the caller cancelled it.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self.inner, Error::Interrupted)
    }

    /// The Engine's own error behind the boundary one, for a test that
    /// matches on its variant.
    #[cfg(test)]
    pub(crate) fn inner(&self) -> &Error {
        &self.inner
    }

    /// Returns the originating error this failure keeps, when it has one.
    ///
    /// A Lua failure keeps the Lua error, a tool failure the tool's own
    /// error, a store failure the store's structured error, and a
    /// frontmatter failure the YAML decode failure. The message already
    /// holds the cause's text, so [`std::error::Error::source`] does not
    /// return it again, and a printer that walks the source chain shows the
    /// text once.
    #[must_use]
    pub fn cause(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.inner.cause()
    }

    /// Returns `true` when retrying the run may succeed (transient transport or
    /// backend failures).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match &self.inner {
            Error::Completion(error) => error.is_retryable(),
            Error::ParseFrontmatter { .. }
            | Error::ParseStructured { .. }
            | Error::Interrupted
            | Error::Lua(_)
            | Error::LuaRuntime { .. }
            | Error::LuaCompile { .. }
            | Error::BindSchema { .. }
            | Error::Substitution(_)
            | Error::ToolLoopExhausted
            | Error::TasksLive { .. }
            | Error::TaskNotOwned { .. }
            | Error::TaskConsumed { .. }
            | Error::TaskCancelled { .. }
            | Error::OutOfScopeToolCall { .. }
            | Error::UnboundToolCall { .. }
            | Error::ModelRequired { .. }
            | Error::UnsupportedVersion(_)
            | Error::RequirementsUnmet { .. }
            | Error::Tool { .. }
            | Error::Internal { .. }
            | Error::LuaQuota { .. }
            | Error::CallDepthExceeded { .. }
            | Error::ContextExhausted { .. }
            | Error::Store { .. }
            | Error::Determinism(_) => false,
        }
    }

    /// Returns where the failure occurred, or `None` when it has no location.
    ///
    /// A failure of kind `Parse` returns its position in the prompt source.
    /// The path is the prompt's frontmatter name when parsing got that far,
    /// and the placeholder `<prompt>` otherwise. The line and column come
    /// from the YAML error for a frontmatter failure, or from the span for
    /// any other parse failure. A failure of kind `Internal` returns the Rust
    /// source file and line of the broken invariant. Failures of every other
    /// kind return `None`.
    #[must_use]
    pub fn location(&self) -> Option<SourceLocation> {
        match &self.inner {
            Error::ParseFrontmatter { line, column, .. } => Some(
                SourceLocation::new("<prompt>")
                    .with_line(*line)
                    .with_column(*column),
            ),
            Error::ParseStructured {
                name,
                line,
                column,
                span,
                ..
            } => Some(
                SourceLocation::new(name.as_deref().unwrap_or("<prompt>"))
                    .with_line(*line)
                    .with_column(*column)
                    .with_span(span.map(|(start, end)| start..end)),
            ),
            Error::Internal { file, line, .. } => {
                Some(SourceLocation::new(*file).with_line(Some(*line)))
            }
            Error::Completion(_)
            | Error::Interrupted
            | Error::Lua(_)
            | Error::LuaRuntime { .. }
            | Error::LuaCompile { .. }
            | Error::BindSchema { .. }
            | Error::Substitution(_)
            | Error::ToolLoopExhausted
            | Error::TasksLive { .. }
            | Error::TaskNotOwned { .. }
            | Error::TaskConsumed { .. }
            | Error::TaskCancelled { .. }
            | Error::OutOfScopeToolCall { .. }
            | Error::UnboundToolCall { .. }
            | Error::ModelRequired { .. }
            | Error::UnsupportedVersion(_)
            | Error::RequirementsUnmet { .. }
            | Error::Tool { .. }
            | Error::LuaQuota { .. }
            | Error::CallDepthExceeded { .. }
            | Error::ContextExhausted { .. }
            | Error::Store { .. }
            | Error::Determinism(_) => None,
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
