//! The parser's internal error type and its public classification.
//!
//! [`Error`] is the internal error type every parsing module returns
//! through [`Result`]. [`ParseError`] is the host-facing wrapper returned
//! by [`Prompt::parse`](crate::Prompt::parse): it classifies the internal
//! type into a stable [`ParseErrorKind`] and surfaces the failure's
//! location fields.

/// A type-erased owned error cause used by the internal error type.
pub(crate) type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// The parser's internal error type, classified into [`ParseError`] at
/// the public boundary.
///
/// Public only so `promptforge-engine` can convert it back onto its own
/// internal type variant-for-variant; the facade does not re-export it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The prompt frontmatter was not valid YAML, preserving the decode
    /// failure as the `#[source]` cause so [`ParseError`] can expose the
    /// frontmatter syntax location through [`std::error::Error::source`].
    #[error("invalid frontmatter: {message}")]
    ParseFrontmatter {
        /// The human-readable diagnostic (no raw source dump).
        message: String,
        /// The originating YAML parse failure, kept as the cause.
        #[source]
        source: BoxedSource,
        /// The 1-based file line of the YAML failure, surfaced from the
        /// retained cause's location when it has one.
        line: Option<u32>,
        /// The 1-based file column of the YAML failure, when known.
        column: Option<u32>,
    },

    /// A structurally-classified parse failure with a stable kind and an
    /// optional source byte span, so [`ParseError`] can expose the
    /// classification and location from stored fields instead of inferring
    /// them from message text.
    #[error("{message}")]
    ParseStructured {
        /// The stable classification of this parse failure.
        kind: ParseErrorKind,
        /// The byte span of the offending region within the source, when known.
        span: Option<(usize, usize)>,
        /// The human-readable diagnostic.
        message: String,
        /// The prompt's frontmatter name, stamped when the failure postdates
        /// the frontmatter (a frontmatter failure predates the name).
        name: Option<String>,
        /// The 1-based file line of the span's start, computed against the
        /// source when a span is known.
        line: Option<u32>,
        /// The 1-based byte column of the span's start, when a span is known.
        column: Option<u32>,
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

    /// Stamps a structured parse failure with the prompt's frontmatter name
    /// and, when the failure has a source span, the span's 1-based
    /// file line and byte column. Every other variant passes through
    /// unchanged: a frontmatter failure predates the name, and a Lua
    /// compile failure already reports its own position.
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
                let (line, column) = match span {
                    Some((start, _)) => body_line_column(body, start, frontmatter_lines),
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
            other => other,
        }
    }
}

/// The 1-based file line and byte column of `byte_offset` within `body`,
/// offset past the frontmatter lines. Both are `None` when the offset is
/// out of bounds or the arithmetic overflows - an invariant break that must
/// not replace the original parse failure.
fn body_line_column(
    body: &str,
    byte_offset: usize,
    frontmatter_lines: u32,
) -> (Option<u32>, Option<u32>) {
    let Some(prefix) = body.get(..byte_offset) else {
        return (None, None);
    };
    let line_in_body = u32::try_from(prefix.matches('\n').count())
        .ok()
        .and_then(|newlines| newlines.checked_add(1));
    let line = line_in_body.and_then(|line| frontmatter_lines.checked_add(line));
    let column = u32::try_from(prefix.len() - prefix.rfind('\n').map_or(0, |index| index + 1))
        .ok()
        .and_then(|offset| offset.checked_add(1));
    (line, column)
}

/// A stable, matchable classification of a [`ParseError`].
///
/// `#[non_exhaustive]` so new kinds do not break a caller's `match`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    /// The YAML frontmatter block was missing, unclosed, or invalid.
    Frontmatter,
    /// The document structure was invalid (missing/duplicate H1, no sections).
    Structure,
    /// A reserved `lua`/`lua shared` fence was misplaced, or an exact fence
    /// was not closed.
    Fence,
    /// A list-only section contained non-list or empty items.
    List,
    /// A compiled Lua region was not syntactically valid.
    Lua,
}

/// The error returned by [`Prompt::parse`](crate::Prompt::parse).
///
/// Holds a stable [`kind`](ParseError::kind) classifier and preserves the
/// underlying cause through [`std::error::Error::source`]. `#[non_exhaustive]`
/// and not constructible outside the crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct ParseError {
    kind: ParseErrorKind,
    span: Option<(usize, usize)>,
    name: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    inner: Box<Error>,
}

/// The classified parts of an internal error: the stable kind plus the
/// location fields the internal error holds (the source span, the prompt's
/// frontmatter name when the failure postdates the frontmatter, and the
/// 1-based line/column - surfaced from the retained YAML failure, or
/// computed from the span).
struct Classification {
    kind: ParseErrorKind,
    span: Option<(usize, usize)>,
    name: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
}

/// Classifies an internal error into its stable kind and location fields.
fn classify_parse_error(inner: &Error) -> Classification {
    const NONE: Classification = Classification {
        kind: ParseErrorKind::Structure,
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
            span: *span,
            name: name.clone(),
            line: *line,
            column: *column,
        },
        Error::ParseFrontmatter { line, column, .. } => Classification {
            kind: ParseErrorKind::Frontmatter,
            line: *line,
            column: *column,
            ..NONE
        },
        Error::Lua(promptforge_lua::Error::LuaCompile { .. }) => Classification {
            kind: ParseErrorKind::Lua,
            ..NONE
        },
        _ => NONE,
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
    /// Structural failures that can locate the offending region (for example a
    /// duplicate sibling section) have a byte span; others return `None`.
    #[must_use]
    pub fn span(&self) -> Option<(usize, usize)> {
        self.span
    }

    /// Returns the prompt's frontmatter name when the failure postdates the
    /// frontmatter.
    ///
    /// A frontmatter YAML failure predates the name (the parser learns the
    /// name from the frontmatter itself), so it reports `None` and the
    /// host's own label for the source takes its place.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns the 1-based file line of the failure, when known.
    ///
    /// Frontmatter failures surface the retained YAML error's position;
    /// structured failures with a source span report the span's start line.
    #[must_use]
    pub fn line(&self) -> Option<u32> {
        self.line
    }

    /// Returns the 1-based column of the failure, when known.
    #[must_use]
    pub fn column(&self) -> Option<u32> {
        self.column
    }

    /// Unwraps the internal error; the engine reaches it through
    /// [`crate::detail::parse_error_into_inner`].
    #[must_use]
    pub(crate) fn into_inner(self) -> Error {
        *self.inner
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.inner)
    }
}

impl From<Error> for ParseError {
    fn from(inner: Error) -> Self {
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
