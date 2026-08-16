//! Prompt file parser.
//!
//! A prompt is one markdown file: YAML frontmatter, a required H1, optional H1
//! blocks and one optional `lua shared` library fence, then H2 sections.
//! H1 and section content are alternating sequences of exact `lua` fences and
//! prose ([`Block`]). Sections nest recursively (H3 under H2, H4 under H3, and
//! so on through H6). The last prose block is marked loop-capable at parse time.
//! Classic prologue/prose/epilog is exactly `[Lua, Prose, Lua]`.
//!
//! The parser does no execution. It turns bytes into a [`Prompt`] tree.

use crate::observe::{Observer, detail};
use crate::{Error, Result};

pub use crate::lua::LuaProgram;

mod build;
mod fence;
mod list;

pub use build::{FileDecl, Frontmatter, MAX_TOOL_ITERATIONS, MaxToolIterations, promptforge_version};
use build::{Heading, build_sections, collect_headings, line_add, split_frontmatter};
use fence::{exact_shared_openings, split_h1};

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
    inner: Box<Error>,
}

/// Classify a substrate error into a stable [`ParseErrorKind`] and optional
/// source span.
///
/// A structured parse fault carries both directly; older string parse errors
/// are classified by message once, here, rather than on every `kind()` call.
fn classify_parse_error(inner: &Error) -> (ParseErrorKind, Option<(usize, usize)>) {
    match inner {
        Error::ParseStructured { kind, span, .. } => (*kind, *span),
        Error::ParseFrontmatter { .. } => (ParseErrorKind::Frontmatter, None),
        Error::LuaCompile { .. } => (ParseErrorKind::Lua, None),
        Error::Parse(message) => {
            let kind = if message.contains("frontmatter") {
                ParseErrorKind::Frontmatter
            } else if message.contains("fence") {
                ParseErrorKind::Fence
            } else if message.contains("list section") || message.contains("bullet item") {
                ParseErrorKind::List
            } else {
                ParseErrorKind::Structure
            };
            (kind, None)
        }
        _ => (ParseErrorKind::Structure, None),
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
        let (kind, span) = classify_parse_error(&inner);
        ParseError {
            kind,
            span,
            inner: Box::new(inner),
        }
    }
}

impl From<ParseError> for Error {
    fn from(error: ParseError) -> Self {
        *error.inner
    }
}

/// One executable block inside a section: a compiled Lua fence or prose.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Block {
    /// An exact `lua` fence compiled at parse time.
    Lua(LuaProgram),
    /// Author prose for the model. `loop_capable` is true only for the last
    /// prose block in the section (full tool loop); earlier prose is single-shot.
    #[non_exhaustive]
    Prose {
        /// Substituted and sent to the model when non-empty.
        text: String,
        /// Whether this prose runs the full tool loop (`true`) or one round.
        loop_capable: bool,
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

    /// Text of the final (loop-capable) prose block, or `""` when absent.
    #[must_use]
    pub fn prose(&self) -> &str {
        self.blocks
            .iter()
            .rev()
            .find_map(|block| match block {
                Block::Prose {
                    text,
                    loop_capable: true,
                } => Some(text.as_str()),
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
    /// use promptforge_core::observe::NullObserver;
    /// use promptforge_core::parser::{Prompt, ParseErrorKind};
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
    /// # Ok::<(), promptforge_core::parser::ParseError>(())
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
            // Retain the YAML decode failure as the `#[source]` cause (F3) so the
            // public parse error can expose the frontmatter syntax location.
            Error::ParseFrontmatter {
                message: e.to_string(),
                source: Box::new(e),
            }
        })?;

        let headings = collect_headings(&body)?;

        let h1_positions: Vec<usize> = headings
            .iter()
            .enumerate()
            .filter_map(|(index, heading)| (heading.level == 1).then_some(index))
            .collect();
        let [h1_index] = h1_positions.as_slice() else {
            return Err(Error::Parse(if h1_positions.is_empty() {
                "prompt requires an H1 title".into()
            } else {
                "prompt must contain exactly one H1 title".into()
            }));
        };
        let h1 = &headings[*h1_index];
        if h1.title.trim().is_empty() {
            return Err(Error::Parse("prompt H1 title must not be empty".into()));
        }
        let title = h1.title.clone();
        let h1_content_abs_line = line_add(frontmatter_lines, h1.content_start_line)?;
        let shared_fences = exact_shared_openings(&body);
        let h1_shared_fences = exact_shared_openings(&h1.content);
        if shared_fences.len() > 1 {
            return Err(Error::Parse(
                "prompt allows at most one `lua shared` fence".into(),
            ));
        }
        if shared_fences.len() != h1_shared_fences.len() {
            return Err(Error::Parse(
                "`lua shared` fence is allowed only in H1".into(),
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
