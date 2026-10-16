//! Defines the parser's internal error type and its public classification.
//!
//! [`Error`] is the internal error type every parsing module returns
//! through [`Result`]. [`ParseError`] is the public wrapper returned
//! by [`Prompt::parse`](crate::Prompt::parse): it classifies the internal
//! type into a stable [`ParseErrorKind`] and surfaces the failure's
//! location fields.
//!
//! A variant that keeps an originating error renders that error's text in
//! its own `Display` and holds the error as a plain `cause` field behind
//! [`Error::cause`], never as a `#[source]`, so a printer that walks the
//! source chain shows the cause's text once.

use std::num::NonZeroU32;
use std::ops::Range;

/// A type-erased owned error cause used by the internal error type.
type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// The parser's internal error type, classified into [`ParseError`] at
/// the public boundary.
///
/// Public only so `promptforge-engine` can convert it back onto its own
/// internal type variant-for-variant; the facade does not re-export it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The prompt frontmatter was not valid YAML.
    ///
    /// Keeps the decode failure as the cause beside the location it reports.
    #[error("invalid frontmatter: {message}")]
    ParseFrontmatter {
        /// The human-readable diagnostic (no raw source dump).
        message: String,
        /// The originating YAML parse failure, whose text `message` holds.
        cause: BoxedSource,
        /// The 1-based file line of the YAML failure, surfaced from the
        /// cause's location when it has one.
        line: Option<NonZeroU32>,
        /// The 1-based file column of the YAML failure, when known.
        column: Option<NonZeroU32>,
    },

    /// A structurally-classified parse failure with a stable kind and an
    /// optional source byte span.
    ///
    /// [`ParseError`] exposes the classification and location from these
    /// stored fields instead of inferring them from message text.
    #[error("{message}")]
    ParseStructured {
        /// The stable classification of this parse failure.
        kind: ParseErrorKind,
        /// The byte span of the offending region within the body (after the
        /// frontmatter and a leading BOM, CRLF normalized to LF), when known.
        span: Option<Range<usize>>,
        /// The human-readable diagnostic.
        message: String,
        /// The prompt's frontmatter name, stamped when the failure postdates
        /// the frontmatter (a frontmatter failure predates the name).
        name: Option<String>,
        /// The 1-based file line of the span's start, computed against the
        /// source when a span is known.
        line: Option<NonZeroU32>,
        /// The 1-based byte column of the span's start, when a span is known.
        column: Option<NonZeroU32>,
    },

    /// A Lua region failed to compile at parse time, preserved as the
    /// `promptforge-lua` internal error type so the compiler diagnostic
    /// chain survives unchanged.
    #[error(transparent)]
    Lua(#[from] promptforge_lua::Error),

    /// An internal parser invariant was violated (a state the surrounding code
    /// has already guaranteed cannot occur).
    #[error("internal invariant violated: {0}")]
    Internal(&'static str),
}

/// The parser's internal result type over [`Error`].
pub(crate) type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Builds a parse failure with a stable classification and no source span.
    pub(crate) fn parse(kind: ParseErrorKind, message: impl Into<String>) -> Error {
        Error::ParseStructured {
            kind,
            span: None,
            message: message.into(),
            name: None,
            line: None,
            column: None,
        }
    }

    /// Builds a parse failure with a stable classification and the byte span
    /// of the offending region within the body.
    ///
    /// [`with_prompt_context`](Error::with_prompt_context) locates the span.
    pub(crate) fn parse_at(
        kind: ParseErrorKind,
        span: Range<usize>,
        message: impl Into<String>,
    ) -> Error {
        Error::ParseStructured {
            kind,
            span: Some(span),
            message: message.into(),
            name: None,
            line: None,
            column: None,
        }
    }

    /// Returns the originating error a cause-bearing failure keeps.
    ///
    /// The failure's `Display` already holds the cause's text. Every other
    /// failure returns `None`.
    #[must_use]
    pub fn cause(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        match self {
            Error::ParseFrontmatter { cause, .. } => Some(cause.as_ref()),
            Error::Lua(lua) => lua.cause(),
            Error::ParseStructured { .. } | Error::Internal(_) => None,
        }
    }

    /// Stamps a structured parse failure with the prompt's frontmatter name
    /// and its span's position.
    ///
    /// The position is the span's 1-based file line and byte column, when
    /// the failure has a span. Every other variant passes through unchanged:
    /// a frontmatter failure predates the name, a Lua compile failure already
    /// reports its own position, and an internal failure has none.
    pub(crate) fn with_prompt_context(
        self,
        name: &str,
        body: &str,
        frontmatter_lines: u32,
    ) -> Error {
        match self {
            Error::ParseStructured {
                kind,
                span,
                message,
                ..
            } => {
                let (line, column) = match &span {
                    Some(span) => body_line_column(body, span.start, frontmatter_lines),
                    None => (None, None),
                };
                Error::ParseStructured {
                    kind,
                    span,
                    message,
                    name: Some(name.to_owned()),
                    line,
                    column,
                }
            }
            other @ (Error::ParseFrontmatter { .. } | Error::Lua(_) | Error::Internal(_)) => other,
        }
    }
}

/// Returns the 1-based file line and byte column of `byte_offset` within
/// `body`, offset past the frontmatter lines.
///
/// Both are `None` when the offset is out of bounds or the arithmetic
/// overflows - an invariant break that must not replace the original parse
/// failure.
fn body_line_column(
    body: &str,
    byte_offset: usize,
    frontmatter_lines: u32,
) -> (Option<NonZeroU32>, Option<NonZeroU32>) {
    let Some(prefix) = body.get(..byte_offset) else {
        return (None, None);
    };
    let line = u32::try_from(prefix.matches('\n').count())
        .ok()
        .and_then(|newlines| NonZeroU32::MIN.checked_add(newlines))
        .and_then(|line| line.checked_add(frontmatter_lines));
    let column = u32::try_from(prefix.len() - prefix.rfind('\n').map_or(0, |index| index + 1))
        .ok()
        .and_then(|offset| NonZeroU32::MIN.checked_add(offset));
    (line, column)
}

/// A stable, matchable classification of a [`ParseError`].
///
/// Adding a new kind is a compatible change, so a `match` on this type
/// needs a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    /// The YAML frontmatter block was missing, left open, or invalid.
    Frontmatter,
    /// The document structure was invalid, for example a missing or duplicate
    /// H1 title, or two sibling sections with the same name.
    Structure,
    /// A reserved `lua` code fence was left open, or a fence or line broke the
    /// strict Lua fence rule.
    ///
    /// The rule breaks are: a fence's label started with `lua` in any letter
    /// case without being exactly `lua` (Label), a `lua` fence's opening line
    /// was not exactly three backticks then `lua` (Opener), its closing line
    /// was not exactly three backticks (Closer), or a line outside every
    /// fence looked like a `lua` opening line (Lookalike). Lines inside the
    /// body of another fenced code block are never inspected.
    Fence,
    /// A list-only section contained prose lines or empty items.
    List,
    /// A Lua region failed to compile because of a syntax error.
    Lua,
    /// The parser failed through no fault of the prompt: one of its own
    /// invariants broke, or the Lua compiler could not start.
    Internal,
}

/// The error returned by [`Prompt::parse`](crate::Prompt::parse).
///
/// It holds a stable [`kind`](ParseError::kind) and, when known, where the
/// failure happened. It keeps the originating error, when there is one,
/// which [`cause`](ParseError::cause) returns.
#[derive(Debug)]
#[non_exhaustive]
pub struct ParseError {
    kind: ParseErrorKind,
    span: Option<Range<usize>>,
    name: Option<String>,
    line: Option<NonZeroU32>,
    column: Option<NonZeroU32>,
    inner: Box<Error>,
}

/// The classified parts of an internal error: the stable kind plus the
/// location fields the internal error holds.
///
/// The location fields are the source span, the prompt's frontmatter name
/// when the failure postdates the frontmatter, and the 1-based line/column -
/// surfaced from the retained YAML failure, or computed from the span.
struct Classification {
    kind: ParseErrorKind,
    span: Option<Range<usize>>,
    name: Option<String>,
    line: Option<NonZeroU32>,
    column: Option<NonZeroU32>,
}

/// Classifies an internal error into its stable kind and location fields.
fn classify_parse_error(inner: &Error) -> Classification {
    let unlocated = |kind| Classification {
        kind,
        span: None,
        name: None,
        line: None,
        column: None,
    };
    match inner {
        Error::ParseStructured {
            kind,
            span,
            name,
            line,
            column,
            ..
        } => Classification {
            kind: *kind,
            span: span.clone(),
            name: name.clone(),
            line: *line,
            column: *column,
        },
        Error::ParseFrontmatter { line, column, .. } => Classification {
            line: *line,
            column: *column,
            ..unlocated(ParseErrorKind::Frontmatter)
        },
        Error::Lua(lua) => unlocated(match lua {
            promptforge_lua::Error::LuaCompile { .. } => ParseErrorKind::Lua,
            // Compiling a block fails otherwise only when the compiler VM
            // cannot start.
            _ => ParseErrorKind::Internal,
        }),
        Error::Internal(_) => unlocated(ParseErrorKind::Internal),
    }
}

impl ParseError {
    /// Returns the stable classification of this failure.
    #[must_use]
    pub fn kind(&self) -> ParseErrorKind {
        self.kind
    }

    /// Returns the byte span of the offending region, when one is available.
    ///
    /// A failure that can locate the offending region, such as a duplicate
    /// sibling section, has a byte span. Other failures return `None`. The
    /// offsets are relative to the document body after the frontmatter and a
    /// leading BOM, with CRLF normalized to LF. To locate the failure in the
    /// original file, use [`line`](ParseError::line) and
    /// [`column`](ParseError::column).
    #[must_use]
    pub fn span(&self) -> Option<Range<usize>> {
        self.span.clone()
    }

    /// Returns the prompt's frontmatter name, when the failure came after the
    /// frontmatter was read.
    ///
    /// A frontmatter failure happens before the parser learns the name, which
    /// comes from the frontmatter itself. It returns `None`, and the caller
    /// can use its own label for the source. A Lua compile failure
    /// also returns `None`.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns the 1-based file line of the failure, when known.
    ///
    /// For a frontmatter YAML failure, this is the line the YAML parser
    /// reported. For a failure with a byte span, it is the line where the
    /// span starts.
    #[must_use]
    pub fn line(&self) -> Option<NonZeroU32> {
        self.line
    }

    /// Returns the 1-based column of the failure, when known.
    #[must_use]
    pub fn column(&self) -> Option<NonZeroU32> {
        self.column
    }

    /// Returns the originating error this failure keeps, when it has one.
    ///
    /// A frontmatter failure keeps the YAML decode failure, and a Lua compile
    /// failure keeps the compiler's own error. The failure's message already
    /// holds the cause's text, so [`std::error::Error::source`] returns
    /// `None` and a printer that walks the source chain shows the text once.
    #[must_use]
    pub fn cause(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.inner.cause()
    }

    /// Unwraps the internal error; the Engine reaches it through
    /// [`crate::detail::parse_error_into_inner`].
    #[must_use]
    pub(crate) fn into_inner(self) -> Error {
        *self.inner
    }

    /// Classifies the internal error into its public wrapper.
    pub(crate) fn from_inner(inner: Error) -> ParseError {
        let classified = classify_parse_error(&inner);
        ParseError {
            kind: classified.kind,
            span: classified.span,
            name: classified.name,
            line: classified.line,
            column: classified.column,
            inner: Box::new(inner),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
#[path = "error-tests.rs"]
mod tests;
