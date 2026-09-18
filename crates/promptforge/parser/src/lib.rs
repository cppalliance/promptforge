//! Prompt file parser.
//!
//! A prompt is one markdown file: YAML frontmatter, a required H1, optional H1
//! blocks and one optional `lua shared` library fence, then H2 sections.
//! H1 and section content are alternating sequences of exact `lua` fences and
//! prose ([`Block`]). Sections nest recursively (H3 under H2, H4 under H3, and
//! so on through H6). Classic prologue/prose/epilog is exactly `[Lua, Prose, Lua]`.
//!
//! Prose capture follows the pending-Markdown model: Markdown accumulates
//! after each heading or ordinary `lua` fence, and each prose block is the
//! pending buffer the following `lua` fence consumes. A `---` thematic break
//! resets the pending buffer without becoming part of the prose; it carries
//! no control-flow meaning, so content below a break parses and runs
//! normally. Markdown left after the final `lua` fence is inert trailing
//! commentary, never an error.
//!
//! The parser does no execution. It turns bytes into a [`Prompt`] tree.

use promptforge_api_types::observe::{Observer, detail};

pub use promptforge_lua::LuaProgram;

mod build;
mod contract;
mod fence;
mod list;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use build::{
    FileDecl, Frontmatter, MAX_TOOL_ITERATIONS, MaxToolIterations, promptforge_version,
};
use build::{Heading, build_sections, collect_headings, line_add, split_frontmatter};
pub use contract::{
    ArgDecl, ArgType, ArgsDecl, CapabilityDecl, ModelKeyword, ModelRole, ModelRoles, ToolSlot,
    ToolSlots,
};
use fence::{exact_shared_openings, split_h1};

/// A type-erased owned error cause used by the internal substrate.
pub(crate) type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// The parser's internal error substrate, classified into [`ParseError`] at
/// the public boundary.
///
/// `#[doc(hidden)]`: this type exists in the public item tree only so the
/// companion `promptforge-api-runtime` crate can convert it back onto its own
/// substrate variant-for-variant. It is not host API.
#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
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
        /// retained cause's location when it carries one.
        line: Option<u32>,
        /// The 1-based file column of the YAML failure, when known.
        column: Option<u32>,
    },

    /// A structurally-classified parse failure carrying a stable kind and an
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

    /// A Lua region failed to compile at parse time, carried as the
    /// `promptforge-lua` substrate so the compiler diagnostic chain survives
    /// unchanged.
    #[error(transparent)]
    Lua(#[from] promptforge_lua::Error),

    /// An internal parser invariant was violated (a state the surrounding code
    /// has already guaranteed cannot occur).
    #[error("internal invariant violated: {0}")]
    Internal(&'static str),
}

/// The parser's internal result type over the [`Error`] substrate.
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
    /// and, when the failure carries a source span, the span's 1-based
    /// file line and byte column. Every other variant passes through
    /// unchanged: a frontmatter failure predates the name, and a Lua
    /// compile failure already carries its own position.
    fn with_prompt_context(self, name: &str, body: &str, frontmatter_lines: u32) -> Error {
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
    /// A reserved `lua`/`lua shared` fence was misplaced or not closed exactly.
    Fence,
    /// A list-only section contained non-list or empty items.
    List,
    /// A compiled Lua region was not syntactically valid.
    Lua,
}

/// The error returned by [`Prompt::parse`].
///
/// Carries a stable [`kind`](ParseError::kind) classifier and preserves the
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

/// The classified parts of a substrate error: the stable kind plus the
/// location fields the substrate carries (the source span, the prompt's
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

/// Classify a substrate error into its stable kind and location fields.
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
    /// duplicate sibling section) carry a byte span; others return `None`.
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
    /// structured failures with a source span carry the span's start line.
    #[must_use]
    pub fn line(&self) -> Option<u32> {
        self.line
    }

    /// Returns the 1-based column of the failure, when known.
    #[must_use]
    pub fn column(&self) -> Option<u32> {
        self.column
    }

    /// Unwraps the internal substrate error.
    ///
    /// `#[doc(hidden)]`: cross-crate seam for `promptforge-api-runtime`'s own error
    /// substrate, mirroring the `promptforge-lua` precedent. Not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn into_inner(self) -> Error {
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

/// One executable block inside a section: a compiled Lua fence or prose.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Block {
    /// An exact `lua` fence compiled at parse time.
    Lua(LuaProgram),
    /// Author prose: the pending Markdown accumulated since the nearest
    /// preceding heading, `lua` fence, or thematic break. The executor
    /// installs it as the following Lua block's lazy `prose` template.
    #[non_exhaustive]
    Prose {
        /// Captured Markdown, trimmed of surrounding blank lines.
        text: String,
    },
}

/// One section of a prompt: a heading, ordered blocks, and children.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Section {
    /// The heading text (the section's address).
    pub(crate) name: String,
    /// The heading level, 2 through 6.
    pub(crate) level: u8,
    /// Ordered lua/prose blocks for this section.
    pub(crate) blocks: Vec<Block>,
    /// Child sections nested under this one (deeper heading levels).
    pub(crate) children: Vec<Section>,
    /// Pre-parsed bullet items for list-only sections (no lua blocks).
    /// Empty for non-list sections.
    pub(crate) items: Vec<String>,
}

impl Section {
    /// Returns the heading text (the section's address).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the heading level (2 through 6).
    #[must_use]
    pub fn level(&self) -> u8 {
        self.level
    }

    /// Returns the ordered Lua and prose blocks of this section.
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Returns the child sections nested under this one.
    #[must_use]
    pub fn children(&self) -> &[Section] {
        &self.children
    }

    /// Returns the pre-parsed bullet items for a list-only section.
    #[must_use]
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Classic leading Lua fence when the first block is Lua.
    #[must_use]
    pub fn prologue(&self) -> Option<&LuaProgram> {
        match self.blocks.first() {
            Some(Block::Lua(program)) => Some(program),
            _ => None,
        }
    }

    /// Text of the last prose block, or `""` when the section has none.
    #[must_use]
    pub fn prose(&self) -> &str {
        self.blocks
            .iter()
            .rev()
            .find_map(|block| match block {
                Block::Prose { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or("")
    }

    /// Classic trailing Lua fence when the last block is Lua and not the sole
    /// leading prologue (a section that is only one Lua block has no epilog).
    #[must_use]
    pub fn epilog(&self) -> Option<&LuaProgram> {
        match self.blocks.as_slice() {
            [Block::Lua(_)] => None,
            [.., Block::Lua(program)] => Some(program),
            _ => None,
        }
    }

    /// True when this section is a validated bullet list.
    ///
    /// A section is list-only exactly when it parsed into non-empty
    /// [`items`](Self::items) - i.e. it had no Lua blocks and every nonblank
    /// prose line was a valid list item (PF-PARSER-005). Ordinary prose (even
    /// prose that happens to contain a single bullet line) is not list-only.
    #[must_use]
    pub fn is_list_only(&self) -> bool {
        !self.items.is_empty()
    }
}

/// A fully parsed prompt file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Prompt {
    /// The parsed YAML frontmatter.
    pub(crate) frontmatter: Frontmatter,
    /// The required H1 title.
    pub(crate) title: String,
    /// The compiled `lua shared` library loaded into section VMs.
    pub(crate) replay: Option<LuaProgram>,
    /// Ordered live Lua and prose blocks from the H1.
    pub(crate) h1_blocks: Vec<Block>,
    /// Human-readable prose from the H1.
    pub(crate) description_text: String,
    /// Top-level sections (H2s) in file order.
    pub(crate) sections: Vec<Section>,
}

impl Prompt {
    /// Returns the parsed frontmatter.
    #[must_use]
    pub fn frontmatter(&self) -> &Frontmatter {
        &self.frontmatter
    }

    /// Returns the required H1 title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the compiled `lua shared` library, when the prompt declares one.
    #[must_use]
    pub fn replay(&self) -> Option<&LuaProgram> {
        self.replay.as_ref()
    }

    /// Returns the ordered live Lua and prose blocks from the H1.
    #[must_use]
    pub fn h1_blocks(&self) -> &[Block] {
        &self.h1_blocks
    }

    /// Returns the top-level H2 sections in file order.
    #[must_use]
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Removes the human-readable prose from the H1, keeping only its live Lua
    /// blocks.
    ///
    /// This is the invariant-preserving replacement for mutating `h1_blocks`
    /// directly: it drops every [`Block::Prose`] from the H1 and clears the
    /// derived description text, leaving the compiled H1 Lua blocks and the rest
    /// of the prompt tree untouched. Callers use it to run a prompt's live H1
    /// resolution without sending any H1 prose to a model.
    pub fn strip_h1_prose(&mut self) {
        self.h1_blocks
            .retain(|block| matches!(block, Block::Lua(_)));
        self.description_text.clear();
    }
}

impl Prompt {
    /// Parse a prompt file's full source text into a [`Prompt`].
    ///
    /// Every parse and compilation report carries the caller-provided
    /// `execution` identifier unchanged.
    ///
    /// ```
    /// use promptforge_api_types::observe::NullObserver;
    /// use promptforge_parser::{Prompt, ParseErrorKind};
    ///
    /// let source = "---\nname: greeter\ndescription: says hi\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n";
    /// let prompt = Prompt::parse(source, "docs", &NullObserver::default())?;
    /// assert_eq!(prompt.frontmatter().name(), "greeter");
    /// assert_eq!(prompt.title(), "Greeter");
    /// assert_eq!(prompt.sections().len(), 1);
    /// assert_eq!(prompt.sections()[0].name(), "Say hi");
    ///
    /// // A malformed prompt reports a classified error.
    /// let err = Prompt::parse("no frontmatter here", "docs", &NullObserver::default()).unwrap_err();
    /// assert_eq!(err.kind(), ParseErrorKind::Frontmatter);
    /// # Ok::<(), promptforge_parser::ParseError>(())
    /// ```
    ///
    /// # Errors
    /// Returns a [`ParseError`] classified `Frontmatter` when the frontmatter
    /// delimiters are missing or the frontmatter is invalid; `Structure` when
    /// the required H1 is missing or the body has no `##` sections; `Fence` when
    /// the H1 opens with the removed `lua prompt` fence form, a reserved fence
    /// is not closed exactly, more than one `lua shared` fence exists, or a
    /// `lua shared` fence is outside H1; and `Lua` when the shared library or an
    /// H1 or section Lua block is not valid Lua.
    pub fn parse(
        input: &str,
        execution: &str,
        observer: &dyn Observer,
    ) -> std::result::Result<Prompt, ParseError> {
        observer.observe(execution, "Prompt", detail::PARSE_STARTED);
        let result = Self::parse_inner(input, execution, observer);
        observer.observe(
            execution,
            "Prompt",
            if result.is_ok() {
                detail::PARSE_SUCCEEDED
            } else {
                detail::PARSE_FAILED
            },
        );
        result.map_err(ParseError::from)
    }

    fn parse_inner(input: &str, execution: &str, observer: &dyn Observer) -> Result<Prompt> {
        let (yaml, body, frontmatter_lines) = split_frontmatter(input)?;
        let frontmatter: Frontmatter = serde_yaml_ng::from_str(&yaml).map_err(|e| {
            // Retain the YAML decode failure as the `#[source]` cause (F3) and
            // surface its location, so the public parse error exposes the
            // frontmatter syntax position as stored fields. The location is
            // relative to the frontmatter block, which starts on file line 2
            // (line 1 is the opening `---` delimiter).
            let (line, column) = e.location().map_or((None, None), |location| {
                (
                    u32::try_from(location.line())
                        .ok()
                        .and_then(|line| line.checked_add(1)),
                    u32::try_from(location.column()).ok(),
                )
            });
            Error::ParseFrontmatter {
                message: e.to_string(),
                source: Box::new(e),
                line,
                column,
            }
        })?;
        // Everything past the frontmatter postdates the prompt's name, so a
        // failure from here on is stamped with it (and its span's position).
        let name = frontmatter.name().to_owned();
        Self::parse_body(frontmatter, &body, frontmatter_lines, execution, observer)
            .map_err(|error| error.with_prompt_context(&name, &body, frontmatter_lines))
    }

    fn parse_body(
        frontmatter: Frontmatter,
        body: &str,
        frontmatter_lines: u32,
        execution: &str,
        observer: &dyn Observer,
    ) -> Result<Prompt> {
        let headings = collect_headings(body)?;

        let h1_positions: Vec<usize> = headings
            .iter()
            .enumerate()
            .filter_map(|(index, heading)| (heading.level == 1).then_some(index))
            .collect();
        let [h1_index] = h1_positions.as_slice() else {
            return Err(Error::parse(
                ParseErrorKind::Structure,
                if h1_positions.is_empty() {
                    "prompt requires an H1 title"
                } else {
                    "prompt must contain exactly one H1 title"
                },
            ));
        };
        let h1 = &headings[*h1_index];
        if h1.title.trim().is_empty() {
            return Err(Error::parse(
                ParseErrorKind::Structure,
                "prompt H1 title must not be empty",
            ));
        }
        let title = h1.title.clone();
        let h1_content_abs_line = line_add(frontmatter_lines, h1.content_start_line)?;
        let shared_fences = exact_shared_openings(body);
        let h1_shared_fences = exact_shared_openings(&h1.content);
        if shared_fences.len() > 1 {
            return Err(Error::parse(
                ParseErrorKind::Fence,
                "prompt allows at most one `lua shared` fence",
            ));
        }
        if shared_fences.len() != h1_shared_fences.len() {
            return Err(Error::parse(
                ParseErrorKind::Fence,
                "`lua shared` fence is allowed only in H1",
            ));
        }
        let (replay, h1_blocks, description_text) = split_h1(
            &h1.content,
            &title,
            h1_content_abs_line,
            execution,
            observer,
        )?;

        // Everything before the H1 is preface and has no prompt semantics.
        // Sections are headings after the H1 at level 2 or deeper.
        let section_headings: Vec<Heading> = headings
            .into_iter()
            .skip(*h1_index + 1)
            .filter(|h| h.level >= 2)
            .collect();
        let mut pos = 0;
        let sections = build_sections(
            &section_headings,
            &mut pos,
            1,
            frontmatter_lines,
            execution,
            observer,
        )?;

        Ok(Prompt {
            frontmatter,
            title,
            replay,
            h1_blocks,
            description_text,
            sections,
        })
    }

    /// The entry-point section: the first top-level section in file order.
    #[must_use]
    pub fn entry(&self) -> Option<&Section> {
        self.sections.first()
    }
}

#[cfg(test)]
mod tests;
