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

use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

pub use crate::lua::LuaProgram;
use crate::observe::{Observer, detail};
use crate::{Error, Result};

mod fence;
mod list;

use fence::{RawBlock, exact_shared_openings, lua_block_location, split_h1, split_section_blocks};
use list::{is_all_list_markers, parse_bullet_items};

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

/// The parsed frontmatter of a prompt file.
///
/// Unknown keys are rejected (`deny_unknown_fields`): a misspelled or
/// unsupported frontmatter field is a prompt authoring error, so it fails at
/// parse rather than being silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Frontmatter {
    /// The prompt's identifier, supplied explicitly by a caller.
    pub(crate) name: String,
    /// One-line description shown in prompt listings and name retrieval.
    pub(crate) description: String,
    /// The promptforge engine major this file targets. Its presence marks the
    /// file as a promptforge prompt; `None` means the file is not one. Optional.
    #[serde(default)]
    pub(crate) promptforge: Option<u32>,
    /// Value returned when the run falls off the last section. Optional.
    #[serde(default)]
    pub(crate) default_return: Option<String>,
    /// Maximum model round trips a section's tool-call loop may take.
    ///
    /// A non-optional [`MaxToolIterations`]: an absent value deserializes to
    /// [`MaxToolIterations::Default`] (the runtime applies its own cap) and any
    /// explicit value is a positive, bounded count. Zero is unrepresentable.
    #[serde(default)]
    pub(crate) max_tool_iterations: MaxToolIterations,
}

/// The largest explicit `max_tool_iterations` a prompt may declare.
pub const MAX_TOOL_ITERATIONS: u32 = 1000;

/// Wraps a computed 1-based source line for [`LuaProgram::compile`].
///
/// Line numbers computed during parsing are always at least 1. A zero here means
/// a broken source-position invariant; rather than silently coercing it to line
/// one, this reports a structured internal parse error.
///
/// # Errors
/// Returns [`Error::Internal`] when `line` is zero.
fn nz_source_line(line: u32) -> Result<std::num::NonZeroU32> {
    std::num::NonZeroU32::new(line).ok_or(Error::Internal(
        "parser: computed 1-based source line was zero",
    ))
}

/// Adds two 1-based line components with overflow checking.
///
/// # Errors
/// Returns [`Error::Internal`] when the sum overflows `u32`, rather than
/// saturating and hiding a broken position.
fn line_add(a: u32, b: u32) -> Result<u32> {
    a.checked_add(b)
        .ok_or(Error::Internal("parser: source line arithmetic overflowed"))
}

/// A frontmatter tool-loop cap that cannot encode zero.
///
/// Frontmatter either omits the cap (the runtime applies its default) or sets a
/// positive, bounded count. Deserialization rejects `0`, negatives, and values
/// above [`MAX_TOOL_ITERATIONS`], so no invalid cap can reach execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum MaxToolIterations {
    /// No explicit cap; the runtime applies its own default.
    #[default]
    Default,
    /// An explicit, positive, bounded cap.
    Limit(std::num::NonZeroU32),
}

impl MaxToolIterations {
    /// Resolves to a concrete iteration cap, using `default` when none was set.
    #[must_use]
    pub fn resolve(self, default: usize) -> usize {
        match self {
            MaxToolIterations::Default => default,
            MaxToolIterations::Limit(limit) => limit.get() as usize,
        }
    }

    /// Returns the explicit limit when one was declared.
    #[must_use]
    pub fn limit(self) -> Option<std::num::NonZeroU32> {
        match self {
            MaxToolIterations::Default => None,
            MaxToolIterations::Limit(limit) => Some(limit),
        }
    }
}

impl<'de> serde::Deserialize<'de> for MaxToolIterations {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Deserialize as i64 so negatives and > u32 values are caught, not wrapped.
        let raw = i64::deserialize(deserializer)?;
        if raw <= 0 {
            return Err(serde::de::Error::custom(format!(
                "max_tool_iterations must be a positive integer (>= 1), got {raw}"
            )));
        }
        if raw > i64::from(MAX_TOOL_ITERATIONS) {
            return Err(serde::de::Error::custom(format!(
                "max_tool_iterations must be <= {MAX_TOOL_ITERATIONS}, got {raw}"
            )));
        }
        // 1 <= raw <= MAX_TOOL_ITERATIONS, so both conversions are infallible.
        let value = u32::try_from(raw)
            .map_err(|_| serde::de::Error::custom("max_tool_iterations is out of range"))?;
        let limit = std::num::NonZeroU32::new(value)
            .ok_or_else(|| serde::de::Error::custom("max_tool_iterations must be non-zero"))?;
        Ok(MaxToolIterations::Limit(limit))
    }
}

impl Frontmatter {
    /// Returns the prompt's caller-supplied identifier.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the one-line description shown in listings.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the declared promptforge engine major, when present.
    #[must_use]
    pub fn promptforge(&self) -> Option<u32> {
        self.promptforge
    }

    /// Returns the value produced when the run falls off the last section.
    #[must_use]
    pub fn default_return(&self) -> Option<&str> {
        self.default_return.as_deref()
    }

    /// Returns the per-section tool-loop cap declared in frontmatter.
    #[must_use]
    pub fn max_tool_iterations(&self) -> MaxToolIterations {
        self.max_tool_iterations
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
    /// let prompt = Prompt::parse(source, "docs", &NullObserver::default()).expect("valid prompt");
    /// assert_eq!(prompt.frontmatter().name(), "greeter");
    /// assert_eq!(prompt.title(), "Greeter");
    /// assert_eq!(prompt.sections().len(), 1);
    /// assert_eq!(prompt.sections()[0].name(), "Say hi");
    ///
    /// // A malformed prompt reports a classified error.
    /// let err = Prompt::parse("no frontmatter here", "docs", &NullObserver::default()).unwrap_err();
    /// assert_eq!(err.kind(), ParseErrorKind::Frontmatter);
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
        let frontmatter: Frontmatter = serde_yaml::from_str(&yaml).map_err(|e| {
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
        if section_headings.is_empty() {
            return Err(Error::Parse("prompt has no ## sections".into()));
        }
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
    pub fn entry(&self) -> &Section {
        // `parse` guarantees `sections` is non-empty.
        &self.sections[0]
    }
}

/// A heading with its title and the prose/Lua that follows it (before the next
/// heading of any level).
#[derive(Debug, Clone)]
struct Heading {
    level: u8,
    title: String,
    content: String,
    /// 1-based line number within `body` where `content` begins.
    content_start_line: u32,
    /// 1-based line number within `body` of the heading line itself.
    source_line: u32,
    /// Byte range of the heading within `body`, carried for span diagnostics.
    span: Range<usize>,
}

/// Split a file into its YAML frontmatter, its markdown body, and the
/// number of lines consumed by the frontmatter block (both `---` delimiters
/// and everything between them).
///
/// The file must open with a `---` line and close the frontmatter with another
/// `---` line. `str::lines` handles both `\n` and `\r\n`.
fn split_frontmatter(input: &str) -> Result<(String, String, u32)> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input); // drop BOM
    let mut lines = input.lines();
    match lines.next() {
        Some(l) if l.trim() == "---" => {}
        _ => {
            return Err(Error::Parse(
                "file must begin with a --- frontmatter delimiter".into(),
            ));
        }
    }
    let mut yaml = String::new();
    let mut closed = false;
    let mut line_count: u32 = 1; // opening ---
    for line in lines.by_ref() {
        line_count += 1;
        if line.trim() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    if !closed {
        return Err(Error::Parse("frontmatter was not closed with ---".into()));
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    Ok((yaml, body, line_count))
}

/// Reports the promptforge engine major declared in `source`'s frontmatter.
///
/// Returns `Some(major)` when `source` opens with a YAML frontmatter block that
/// declares a `promptforge:` key, and `None` otherwise. The check is lenient by
/// design and never errors or panics: a source with no frontmatter block,
/// malformed or unclosed frontmatter, or a frontmatter that simply omits the
/// key all read as `None` ("not a promptforge prompt"). No other frontmatter
/// field is required for detection.
///
/// # Examples
/// ```
/// use promptforge_core::promptforge_version;
///
/// assert_eq!(promptforge_version("---\npromptforge: 1\n---\n\n## S\n\np\n"), Some(1));
/// assert_eq!(promptforge_version("just prose, no frontmatter"), None);
/// ```
#[must_use]
pub fn promptforge_version(source: &str) -> Option<u32> {
    /// Reads only the `promptforge` key, ignoring every other field so
    /// detection does not depend on a complete, valid [`Frontmatter`].
    #[derive(serde::Deserialize)]
    struct Probe {
        #[serde(default)]
        promptforge: Option<u32>,
    }

    let (yaml, _body, _lines) = split_frontmatter(source).ok()?;
    let probe: Probe = serde_yaml::from_str(&yaml).ok()?;
    probe.promptforge
}

/// Counts the number of `\n` characters in `text[..byte_offset]`.
///
/// # Errors
/// Returns [`Error::Internal`] when the newline count exceeds `u32`, rather than
/// saturating to `u32::MAX` and hiding an implausibly large source position.
fn newlines_before(text: &str, byte_offset: usize) -> Result<u32> {
    u32::try_from(text[..byte_offset].matches('\n').count())
        .map_err(|_| Error::Internal("parser: newline count exceeded u32 range"))
}

/// Convert a `HeadingLevel` to its numeric level.
fn level_num(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Walk the markdown body and collect every heading with the content that
/// follows it, up to the next heading of any level.
fn collect_headings(body: &str) -> Result<Vec<Heading>> {
    // First pass: find each heading's level, title, and source byte range.
    struct Raw {
        level: u8,
        title: String,
        range: Range<usize>,
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut current: Option<(u8, Range<usize>, String)> = None;

    for (event, range) in Parser::new_ext(body, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((level_num(level), range.clone(), String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, range, title)) = current.take() {
                    raws.push(Raw {
                        level,
                        title: title.trim().to_string(),
                        range,
                    });
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, _, ref mut title)) = current {
                    title.push_str(&t);
                }
            }
            _ => {}
        }
    }

    // Second pass: the content of heading i runs from the end of its heading to
    // the start of the next heading (or the end of the body).
    let mut headings = Vec::with_capacity(raws.len());
    for i in 0..raws.len() {
        let start = raws[i].range.end;
        let end = raws.get(i + 1).map_or(body.len(), |next| next.range.start);
        let content = &body[start..end];
        // +1 because newlines_before gives 0-based offset, and we want
        // the 1-based line number of the first content line.
        let content_start_line = line_add(newlines_before(body, start)?, 1)?;
        let source_line = line_add(newlines_before(body, raws[i].range.start)?, 1)?;
        headings.push(Heading {
            level: raws[i].level,
            title: raws[i].title.clone(),
            content: content.to_string(),
            content_start_line,
            source_line,
            span: raws[i].range.clone(),
        });
    }
    Ok(headings)
}

/// Build a section tree from a flat, document-ordered list of headings.
///
/// Recursion consumes headings whose level is deeper than `parent_level`; a
/// heading at or above `parent_level` belongs to an ancestor and stops the
/// current level. A heading that skips a level (an orphan deep heading, such as
/// an H4 directly under an H2, or an H3 top-level section with no parent H2) is
/// rejected: every heading must be exactly one level deeper than its parent.
///
/// # Errors
/// Returns [`Error::Parse`] when a heading is more than one level deeper than
/// its parent.
fn build_sections(
    headings: &[Heading],
    pos: &mut usize,
    parent_level: u8,
    frontmatter_lines: u32,
    execution: &str,
    observer: &dyn Observer,
) -> Result<Vec<Section>> {
    let mut result = Vec::new();
    // Parallel to `result`: each sibling's name and its 1-based heading line, so
    // a duplicate can name both the first and the offending location.
    let mut sibling_lines: Vec<(String, u32)> = Vec::new();
    while *pos < headings.len() {
        let level = headings[*pos].level;
        if level <= parent_level {
            break;
        }
        // An orphan deep heading skips a level (e.g. an H4 under an H2 with no
        // intervening H3, or an H3/H4 top-level section with no parent H2). Such
        // a heading has no well-defined parent, so reject it rather than
        // silently reparenting it to a shallower ancestor.
        if level > parent_level + 1 {
            return Err(Error::Parse(format!(
                "section `{}` is an orphan H{level} heading with no parent H{}",
                headings[*pos].title.trim(),
                parent_level + 1
            )));
        }
        let h = &headings[*pos];
        let name = h.title.clone();
        // A section's name is its runtime address (jumps, lookups, fanout), so an
        // empty/whitespace heading is unaddressable and must be rejected at parse.
        if name.trim().is_empty() {
            return Err(Error::Parse(format!(
                "an H{level} section heading must not be empty"
            )));
        }
        let content_abs_line = line_add(frontmatter_lines, h.content_start_line)?;
        let heading_abs_line = line_add(frontmatter_lines, h.source_line)?;
        let heading_span = h.span.clone();
        let raw_blocks = split_section_blocks(&h.content, &name)?;
        let has_prose = raw_blocks
            .iter()
            .any(|block| matches!(block, RawBlock::Prose(_)));
        let last_prose = raw_blocks
            .iter()
            .rposition(|block| matches!(block, RawBlock::Prose(_)));
        let total = raw_blocks.len();
        let mut blocks = Vec::with_capacity(total);
        for (index, raw) in raw_blocks.into_iter().enumerate() {
            match raw {
                RawBlock::Prose(text) => {
                    let loop_capable = Some(index) == last_prose;
                    blocks.push(Block::Prose { text, loop_capable });
                }
                RawBlock::Lua {
                    source,
                    line_offset,
                } => {
                    let abs_line = line_add(content_abs_line, line_offset)?;
                    let location = lua_block_location(&name, index, total, has_prose);
                    let program = LuaProgram::compile(
                        &source,
                        &location,
                        nz_source_line(abs_line)?,
                        execution,
                        observer,
                        &name,
                    )?;
                    blocks.push(Block::Lua(program));
                }
            }
        }
        *pos += 1;
        let children =
            build_sections(headings, pos, level, frontmatter_lines, execution, observer)?;

        let has_no_lua = blocks
            .iter()
            .all(|block| matches!(block, Block::Prose { .. }));
        let prose_for_items = blocks.iter().find_map(|block| match block {
            Block::Prose { text, .. } => Some(text.as_str()),
            Block::Lua(_) => None,
        });
        // A section is a list only when it has no Lua and every nonblank prose
        // line is a list marker; one incidental bullet in mixed prose leaves a
        // non-marker line, so the section stays prose.
        let items = if has_no_lua
            && let Some(prose) = prose_for_items
            && is_all_list_markers(prose)
        {
            parse_bullet_items(prose, &name)?
        } else {
            Vec::new()
        };

        // Sibling sections must have unique names: sections are addressed by
        // name (jumps, lookups), so two siblings sharing a name would make the
        // target ambiguous. Reject the duplicate at parse, naming BOTH heading
        // locations so the author can find each one.
        if let Some((_, first_line)) = sibling_lines.iter().find(|(n, _)| *n == name) {
            return Err(Error::ParseStructured {
                kind: ParseErrorKind::Structure,
                span: Some((heading_span.start, heading_span.end)),
                message: format!(
                    "duplicate sibling section name `{name}`: first declared at line {first_line}, again at line {heading_abs_line}; sibling section names must be unique"
                ),
            });
        }
        sibling_lines.push((name.clone(), heading_abs_line));

        result.push(Section {
            name,
            level,
            blocks,
            children,
            items,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::observe::{NullObserver, Observation, detail};

    #[test]
    fn invalid_frontmatter_preserves_the_yaml_cause_as_source() {
        // error.rs F3: a malformed-YAML frontmatter must classify as
        // `Frontmatter` and retain the underlying serde_yaml failure as the
        // public error's `source()`, instead of flattening it into a string.
        let src = "---\nname: p\ndescription: d\n: : :\n---\n\n# T\n\n## S\n\nhi\n";
        let error = Prompt::parse(src, "test", &NullObserver)
            .expect_err("malformed YAML frontmatter must fail to parse");
        assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
        assert!(
            std::error::Error::source(&error).is_some(),
            "the YAML decode failure must be preserved as the error source: {error}"
        );
    }

    #[test]
    fn mixed_prose_with_one_bullet_is_not_a_list() {
        // PF-PARSER-005: an incidental bullet line in ordinary prose must not
        // force strict list parsing; the section stays prose.
        let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\nHere is context.\n- one incidental bullet\nMore prose follows.\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
        let section = &prompt.sections[0];
        assert!(!section.is_list_only(), "mixed prose is not a list");
        assert!(section.items().is_empty());
        assert!(section.prose().contains("incidental bullet"));
    }

    #[test]
    fn pure_list_section_parses_items() {
        let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\n- alpha\n- beta\n3. gamma\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
        let section = &prompt.sections[0];
        assert!(section.is_list_only());
        assert_eq!(section.items(), ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn all_marker_list_with_empty_item_is_rejected() {
        // Every nonblank line is a marker, so it is a list; the empty marker is
        // then a hard error rather than a detector miss.
        let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\n- alpha\n1.\n- beta\n";
        let error = Prompt::parse(src, "test", &NullObserver).expect_err("empty item must fail");
        assert_eq!(error.kind(), ParseErrorKind::List);
    }

    #[test]
    fn parsed_prompt_value_types_are_equatable() {
        // PF-PARSER-011: parsing the same source twice yields equal values, and
        // a differing source yields unequal values, across the finalized parser
        // value types (`Prompt`, `Frontmatter`, `Section`, `Block`).
        let src = "---\nname: p\ndescription: d\n---\n\n# Title\n\n## One\n\ndo a thing\n";
        let a = Prompt::parse(src, "test", &NullObserver).unwrap();
        let b = Prompt::parse(src, "test", &NullObserver).unwrap();
        assert_eq!(a, b, "identical sources must parse equal");
        assert_eq!(a.frontmatter, b.frontmatter);
        assert_eq!(a.sections, b.sections);

        let other = "---\nname: p\ndescription: d\n---\n\n# Title\n\n## Two\n\ndo a thing\n";
        let c = Prompt::parse(other, "test", &NullObserver).unwrap();
        assert_ne!(a, c, "differing section headings must parse unequal");
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, execution: &str, section: &str, event: Observation) {
            self.0
                .lock()
                .expect("recording lock must remain usable")
                .push((
                    execution.to_string(),
                    section.to_string(),
                    event.to_string(),
                ));
        }
    }

    impl Recorder {
        fn records(&self) -> Vec<(String, String, String)> {
            self.0
                .lock()
                .expect("recording lock must remain usable")
                .clone()
        }

        fn observations(&self) -> Vec<(String, String)> {
            self.0
                .lock()
                .expect("recording lock must remain usable")
                .iter()
                .map(|(_, section, detail)| (section.clone(), detail.clone()))
                .collect()
        }
    }

    #[test]
    fn parses_multi_section_with_all_features() {
        let src = "---\n\
name: demo\n\
description: A demo\n\
---\n\
\n\
# Demo Title\n\
\n\
Human-readable intro text.\n\
\n\
## First\n\
\n\
```lua\n\
local x = 1\n\
```\n\
\n\
Prose for the first section.\n\
\n\
### Child\n\
\n\
Child prose.\n\
\n\
## Second\n\
\n\
Prose for the second section.\n";

        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        assert_eq!(p.frontmatter.name, "demo");
        assert_eq!(p.frontmatter.description, "A demo");
        assert_eq!(p.title, "Demo Title");
        assert!(p.replay.is_none());
        assert_eq!(
            p.h1_blocks,
            vec![Block::Prose {
                text: "Human-readable intro text.".to_owned(),
                loop_capable: true,
            }]
        );
        assert_eq!(p.description_text, "Human-readable intro text.");

        assert_eq!(p.sections.len(), 2);
        let first = &p.sections[0];
        assert_eq!(first.name, "First");
        assert_eq!(first.level, 2);
        assert_eq!(
            first.prologue().map(LuaProgram::source),
            Some("local x = 1")
        );
        assert_eq!(first.prose(), "Prose for the first section.");
        assert!(first.epilog().is_none());
        assert_eq!(first.children.len(), 1);
        assert_eq!(first.children[0].name, "Child");
        assert_eq!(first.children[0].level, 3);
        assert_eq!(first.children[0].prose(), "Child prose.");

        assert_eq!(p.sections[1].name, "Second");
        assert!(p.sections[1].prologue().is_none());
        assert!(p.sections[1].epilog().is_none());
    }

    #[test]
    fn parses_single_minimal_section() {
        let src = "---\nname: hi\ndescription: d\n---\n\n# T\n\n## Greet\n\nSay hi\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        assert_eq!(p.sections.len(), 1);
        assert_eq!(p.sections[0].name, "Greet");
        assert_eq!(p.sections[0].prose(), "Say hi");
        assert!(p.frontmatter.default_return.is_none());
    }

    #[test]
    fn name_and_description_are_sufficient_frontmatter_for_parsing() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("minimum frontmatter must parse");
        assert_eq!(prompt.frontmatter.name, "x");
    }

    #[test]
    fn missing_frontmatter_delimiter_errors() {
        let src = "# T\n\n## S\n\np\n";
        assert!(Prompt::parse(src, "test", &NullObserver).is_err());
    }

    #[test]
    fn no_sections_errors() {
        let src = "---\nname: x\ndescription: d\n---\n\n# Only a title\n\nText.\n";
        assert!(Prompt::parse(src, "test", &NullObserver).is_err());
    }

    #[test]
    fn empty_h1_title_errors() {
        let src = "---\nname: x\ndescription: d\n---\n\n#\n\n## S\n\np\n";
        let error =
            Prompt::parse(src, "test", &NullObserver).expect_err("H1 title must not be empty");
        assert!(error.to_string().contains("title must not be empty"));
    }

    #[test]
    fn preface_before_h1_is_ignored() {
        let src = "---\nname: x\ndescription: d\n---\n\nIgnored preface.\n\n```text\nalso ignored\n```\n\n# T\n\nDescription.\n\n## S\n\np\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("preface is not semantic");
        assert_eq!(prompt.title, "T");
        assert_eq!(prompt.description_text, "Description.");
        assert_eq!(prompt.entry().name, "S");
    }

    #[test]
    fn shared_library_allows_blank_lines_and_is_compiled() {
        let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n \t\r\n```lua shared\r\nfunction answer() return 42 end\r\n```\r\n\r\nDescription.\r\n\r\n## S\r\n\r\np\r\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("shared Lua must parse");
        let replay = prompt.replay.expect("replay program must be present");
        assert_eq!(replay.source(), "function answer() return 42 end");
        assert_eq!(prompt.description_text, "Description.");
    }

    #[test]
    fn h1_plain_lua_and_prose_are_live_blocks() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal first = 1\n```\n\nPlan {{ args }}.\n\n```lua shared\nfunction helper() return 1 end\n```\n\n```lua\nstore.write('done', reply)\n```\n\n## S\n\np\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("H1 blocks must parse");
        assert_eq!(
            prompt.replay.as_ref().map(LuaProgram::source),
            Some("function helper() return 1 end")
        );
        assert_eq!(prompt.h1_blocks.len(), 3);
        assert!(matches!(
            &prompt.h1_blocks[0],
            Block::Lua(program) if program.source() == "local first = 1"
        ));
        assert!(matches!(
            &prompt.h1_blocks[1],
            Block::Prose {
                text,
                loop_capable: true
            } if text == "Plan {{ args }}."
        ));
        assert!(matches!(
            &prompt.h1_blocks[2],
            Block::Lua(program) if program.source() == "store.write('done', reply)"
        ));
        assert_eq!(prompt.description_text, "Plan {{ args }}.");
    }

    #[test]
    fn lone_plain_h1_lua_is_not_a_shared_library() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal live = true\n```\n\n## S\n\np\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("plain H1 Lua must parse");
        assert!(prompt.replay.is_none());
        assert!(matches!(
            prompt.h1_blocks.as_slice(),
            [Block::Lua(program)] if program.source() == "local live = true"
        ));
    }

    #[test]
    fn second_shared_fence_is_a_parse_error() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nlocal a = 1\n```\n\n```lua shared\nlocal b = 2\n```\n\n## S\n\np\n";
        let error =
            Prompt::parse(src, "test", &NullObserver).expect_err("a second shared fence must fail");
        assert!(error.to_string().contains("at most one `lua shared`"));
    }

    #[test]
    fn shared_fence_in_h2_is_a_parse_error() {
        let src =
            "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua shared\nlocal a = 1\n```\n";
        let error =
            Prompt::parse(src, "test", &NullObserver).expect_err("a shared fence in H2 must fail");
        assert!(error.to_string().contains("allowed only in H1"));
    }

    #[test]
    fn removed_lua_prompt_form_is_a_targeted_error_when_leading() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua prompt\nlocal a = 1\n```\n\n## S\n\np\n";
        let error = Prompt::parse(src, "test", &NullObserver)
            .expect_err("the removed leading form must be rejected by name");
        assert!(
            error
                .to_string()
                .contains("`lua prompt` fence form was removed")
        );
    }

    #[test]
    fn lua_prompt_form_after_prose_is_ordinary_prose() {
        let in_h1 = "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro.\n\n```lua prompt\nnot compiled =\n```\n\n## S\n\np\n";
        let prompt = Prompt::parse(in_h1, "test", &NullObserver)
            .expect("the removed form after prose is ordinary Markdown");
        assert!(prompt.replay.is_none());
        assert!(prompt.description_text.contains("```lua prompt"));

        let in_section = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua prompt\nnot compiled =\n```\n";
        let prompt = Prompt::parse(in_section, "test", &NullObserver)
            .expect("the removed form in a section is ordinary Markdown");
        assert!(prompt.entry().prologue().is_none());
        assert!(prompt.entry().prose().contains("```lua prompt"));
    }

    #[test]
    fn shared_fence_markers_must_be_exact() {
        // Only the exact ```lua shared opener is reserved, so each near-miss
        // remains H1 prose.
        // The removed ```lua prompt form is excluded because leading it is a
        // targeted error, pinned by
        // `removed_lua_prompt_form_is_a_targeted_error_when_leading`.
        for near_miss in [
            "````lua shared\nreturn 1\n````",
            " ```lua shared\nreturn 1\n ```",
            "```Lua shared\nreturn 1\n```",
            "```lua  shared\nreturn 1\n```",
            "```lua shared extra\nreturn 1\n```",
        ] {
            let src =
                format!("---\nname: x\ndescription: d\n---\n\n# T\n\n{near_miss}\n\n## S\n\np\n");
            let prompt = Prompt::parse(&src, "test", &NullObserver)
                .expect("leading near-miss shared markers must remain prose");
            assert!(prompt.replay.is_none());
            assert!(prompt.description_text.contains(near_miss.trim()));
        }

        // Placement does not change exact-marker recognition.
        for near_miss in [
            "````lua shared\nreturn 1\n````",
            " ```lua shared\nreturn 1\n ```",
            "```Lua shared\nreturn 1\n```",
            "```lua shared extra\nreturn 1\n```",
            "```lua prompt\nreturn 1\n```",
        ] {
            let src = format!(
                "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro.\n\n{near_miss}\n\n## S\n\np\n"
            );
            let prompt = Prompt::parse(&src, "test", &NullObserver)
                .expect("near-miss shared markers must remain prose");
            assert!(prompt.replay.is_none());
            assert!(prompt.description_text.contains(near_miss));
        }

        let unclosed = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nreturn 1\n````\n\n## S\n\np\n";
        let error = Prompt::parse(unclosed, "test", &NullObserver)
            .expect_err("near-miss closing marker must not close the fence");
        assert!(error.to_string().contains("not closed"));
    }

    #[test]
    fn shared_markers_inside_longer_fences_remain_prose() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n````markdown\n```lua shared\nreturn 1\n```\n````\n\nIntro.\n\n## S\n\n````markdown\n```lua shared\nreturn 2\n```\n````\n";
        let prompt = Prompt::parse(src, "test", &NullObserver)
            .expect("nested shared markers must remain prose");

        assert!(prompt.replay.is_none());
        assert!(prompt.description_text.contains("```lua shared"));
        assert!(prompt.sections[0].prologue().is_none());
        assert!(prompt.sections[0].prose().contains("```lua shared"));
    }

    #[test]
    fn malformed_shared_lua_retains_diagnostics_and_reports_safe_boundaries() {
        let recorder = Recorder::default();
        let source = "private_payload =";
        let src = format!(
            "---\nname: x\ndescription: d\n---\n\n# Private title\n\n```lua shared\n{source}\n```\n\n## S\n\np\n"
        );
        let error = Prompt::parse(&src, "parse-failure", &recorder)
            .expect_err("malformed shared Lua must fail");
        match Error::from(error) {
            Error::LuaCompile {
                location,
                lua_source,
                ..
            } => {
                assert_eq!(location, "prompt shared library");
                assert_eq!(lua_source, source);
            }
            other => panic!("expected LuaCompile, got {other:?}"),
        }
        let observations = recorder.observations();
        assert_eq!(
            observations,
            vec![
                ("Prompt".into(), detail::PARSE_STARTED.to_string()),
                (
                    "Private title".into(),
                    detail::LUA_COMPILATION_STARTED.to_string()
                ),
                (
                    "Private title".into(),
                    detail::LUA_COMPILATION_FAILED.to_string()
                ),
                ("Prompt".into(), detail::PARSE_FAILED.to_string()),
            ]
        );
        assert!(
            observations
                .iter()
                .all(|(section, detail)| !section.contains(source) && !detail.contains(source))
        );
    }

    #[test]
    fn successful_parse_reports_only_fixed_boundaries() {
        let recorder = Recorder::default();
        let source = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal secret = 42\n```\n\n## S\n\np\n";
        Prompt::parse(source, "parse-success", &recorder).expect("prompt must parse");
        assert!(
            recorder
                .records()
                .iter()
                .all(|(execution, _, _)| execution == "parse-success")
        );
        assert_eq!(
            recorder.observations(),
            vec![
                ("Prompt".into(), detail::PARSE_STARTED.to_string()),
                ("T".into(), detail::LUA_COMPILATION_STARTED.to_string()),
                ("T".into(), detail::LUA_COMPILATION_SUCCEEDED.to_string()),
                ("Prompt".into(), detail::PARSE_SUCCEEDED.to_string()),
            ]
        );
    }

    #[test]
    fn lua_fence_separated_from_prose() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nreturn 42\n```\n\nActual prose here.\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        assert_eq!(
            p.sections[0].prologue().map(LuaProgram::source),
            Some("return 42")
        );
        assert_eq!(p.sections[0].prose(), "Actual prose here.");
        assert!(p.sections[0].epilog().is_none());
    }

    #[test]
    fn section_compiles_prologue_and_epilog_around_prose() {
        let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n## Transform\r\n\r\n \t\r\n```lua\r\nvar.before = args\r\n```\r\n\r\nAsk about {{ var.before }}.\r\n\r\n```lua\r\nreturn reply\r\n```\r\n";
        let prompt = Prompt::parse(src, "test", &NullObserver)
            .expect("both exact section phases must compile");
        let section = prompt.entry();

        assert_eq!(
            section.prologue().map(LuaProgram::source),
            Some("var.before = args")
        );
        assert_eq!(section.prose(), "Ask about {{ var.before }}.");
        assert_eq!(
            section.epilog().map(LuaProgram::source),
            Some("return reply")
        );
    }

    #[test]
    fn section_compiles_epilog_after_prose_without_prologue() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Transform\n\nAsk the model.\n\n```lua\nreturn reply\n```\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("the trailing epilog must compile");
        let section = prompt.entry();

        assert!(section.prologue().is_none());
        assert_eq!(section.prose(), "Ask the model.");
        assert_eq!(
            section.epilog().map(LuaProgram::source),
            Some("return reply")
        );
    }

    #[test]
    fn exact_middle_lua_fences_become_compiled_blocks() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nBefore.\n\n```lua\nvar.mid = 1\n```\n\nAfter.\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("middle Lua fences compile as blocks");
        let section = prompt.entry();

        assert!(section.prologue().is_none());
        assert!(section.epilog().is_none());
        assert_eq!(section.prose(), "After.");
        assert_eq!(section.blocks.len(), 3);
        match &section.blocks[0] {
            Block::Prose {
                text,
                loop_capable: false,
            } => assert_eq!(text, "Before."),
            other => panic!("expected non-final prose, got {other:?}"),
        }
        match &section.blocks[1] {
            Block::Lua(program) => assert_eq!(program.source(), "var.mid = 1"),
            other => panic!("expected lua block, got {other:?}"),
        }
        match &section.blocks[2] {
            Block::Prose {
                text,
                loop_capable: true,
            } => assert_eq!(text, "After."),
            other => panic!("expected final prose, got {other:?}"),
        }
    }

    #[test]
    fn invalid_middle_lua_fence_fails_parse() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nBefore.\n\n```lua\nnot valid lua =\n```\n\nAfter.\n";
        let err = Prompt::parse(src, "test", &NullObserver)
            .expect_err("invalid middle Lua must fail compilation");
        assert!(
            err.to_string().contains("lua") || err.to_string().contains("Lua"),
            "error was: {err}"
        );
    }

    #[test]
    fn one_exact_fence_is_the_prologue_and_two_can_surround_empty_prose() {
        let one = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.x = 1\n```\n";
        let prompt = Prompt::parse(one, "test", &NullObserver).expect("one fence is the prologue");
        assert!(prompt.entry().prologue().is_some());
        assert!(prompt.entry().epilog().is_none());

        let two = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.x = 1\n```\n\n```lua\nreturn reply\n```\n";
        let prompt =
            Prompt::parse(two, "test", &NullObserver).expect("two fences can enclose empty prose");
        assert_eq!(prompt.entry().prose(), "");
        assert!(prompt.entry().prologue().is_some());
        assert!(prompt.entry().epilog().is_some());
    }

    #[test]
    fn section_fence_markers_must_be_exact() {
        for near_miss in [
            "````lua\nreturn 1\n````",
            " ```lua\nreturn 1\n ```",
            "```Lua\nreturn 1\n```",
            "```lua extra\nreturn 1\n```",
        ] {
            let src = format!("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{near_miss}\n");
            let prompt = Prompt::parse(&src, "test", &NullObserver)
                .expect("near-miss fence must remain prose");
            assert!(prompt.entry().prologue().is_none());
            assert!(prompt.entry().epilog().is_none());
            assert_eq!(prompt.entry().prose(), near_miss.trim());
        }
    }

    #[test]
    fn section_markers_inside_longer_fences_remain_prose() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n````markdown\n```lua\nreturn 1\n```\n````\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("nested markers must remain prose");

        assert!(prompt.entry().prologue().is_none());
        assert!(prompt.entry().epilog().is_none());
        assert!(prompt.entry().prose().contains("```lua"));
    }

    #[test]
    fn malformed_section_phases_report_locations_and_safe_boundaries() {
        for (phase, content, expected_location, expected_details) in [
            (
                "prologue",
                "```lua\nprivate_payload =\n```\n\nProse.",
                "section `Private section` prologue",
                vec![
                    detail::PARSE_STARTED,
                    detail::LUA_COMPILATION_STARTED,
                    detail::LUA_COMPILATION_FAILED,
                    detail::PARSE_FAILED,
                ],
            ),
            (
                "epilog",
                "Prose.\n\n```lua\nprivate_payload =\n```",
                "section `Private section` epilog",
                vec![
                    detail::PARSE_STARTED,
                    detail::LUA_COMPILATION_STARTED,
                    detail::LUA_COMPILATION_FAILED,
                    detail::PARSE_FAILED,
                ],
            ),
        ] {
            let recorder = Recorder::default();
            let src = format!(
                "---\nname: x\ndescription: d\n---\n\n# T\n\n## Private section\n\n{content}\n"
            );
            let Err(error) = Prompt::parse(&src, "test", &recorder) else {
                panic!("malformed {phase} unexpectedly parsed");
            };
            match Error::from(error) {
                Error::LuaCompile {
                    location,
                    lua_source,
                    ..
                } => {
                    assert_eq!(location, expected_location);
                    assert_eq!(lua_source, "private_payload =");
                }
                other => panic!("expected LuaCompile, got {other:?}"),
            }

            let observations = recorder.observations();
            assert_eq!(
                observations
                    .iter()
                    .map(|(_, detail)| detail.clone())
                    .collect::<Vec<_>>(),
                expected_details
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
            );
            assert!(
                observations
                    .iter()
                    .all(|(_, detail)| !detail.contains("private_payload"))
            );
        }
    }

    #[test]
    fn unclosed_reserved_section_fences_are_location_errors() {
        for (content, phase) in [
            ("```lua\nreturn 1", "prologue"),
            ("Prose.\n\n```lua\nreturn reply", "epilog"),
        ] {
            let src = format!("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{content}\n");
            let error = Prompt::parse(&src, "test", &NullObserver)
                .expect_err("reserved fence must close exactly");
            assert!(error.to_string().contains(phase));
            assert!(error.to_string().contains("not closed"));
        }
    }

    #[test]
    fn successful_section_compilation_reports_fixed_ordered_boundaries() {
        let recorder = Recorder::default();
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.secret = 1\n```\n\nProse.\n\n```lua\nreturn reply\n```\n";
        Prompt::parse(src, "section-programs", &recorder).expect("section programs must compile");

        assert_eq!(
            recorder.observations(),
            vec![
                ("Prompt".into(), detail::PARSE_STARTED.to_string()),
                ("S".into(), detail::LUA_COMPILATION_STARTED.to_string()),
                ("S".into(), detail::LUA_COMPILATION_SUCCEEDED.to_string()),
                ("S".into(), detail::LUA_COMPILATION_STARTED.to_string()),
                ("S".into(), detail::LUA_COMPILATION_SUCCEEDED.to_string()),
                ("Prompt".into(), detail::PARSE_SUCCEEDED.to_string()),
            ]
        );
    }

    #[test]
    fn non_lua_fence_stays_in_prose() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nHere is code:\n\n```python\nprint(1)\n```\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        assert!(p.sections[0].prologue().is_none());
        assert!(p.sections[0].epilog().is_none());
        assert!(p.sections[0].prose().contains("```python"));
    }

    #[test]
    fn recursive_nesting_h2_h3_h4() {
        let src =
            "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n### B\n\nb\n\n#### C\n\nc\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        let a = &p.sections[0];
        assert_eq!(a.name, "A");
        let b = &a.children[0];
        assert_eq!(b.name, "B");
        assert_eq!(b.level, 3);
        let c = &b.children[0];
        assert_eq!(c.name, "C");
        assert_eq!(c.level, 4);
    }

    #[test]
    fn skipped_heading_level_is_rejected_as_orphan() {
        // H4 directly under H2 (no intervening H3) is an orphan deep heading:
        // it has no parent H3, so it must be rejected, not reparented to the H2.
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n#### D\n\nd\n";
        let err = Prompt::parse(src, "test", &NullObserver)
            .expect_err("an H4 with no parent H3 must be rejected");
        assert!(
            err.to_string().contains("orphan"),
            "expected an orphan-heading error, got: {err}"
        );
    }

    #[test]
    fn orphan_top_level_deep_heading_is_rejected() {
        // The first section heading is an H3 with no parent H2: an orphan.
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n### A\n\na\n";
        let err = Prompt::parse(src, "test", &NullObserver)
            .expect_err("an H3 top-level section with no parent H2 must be rejected");
        assert!(
            err.to_string().contains("orphan"),
            "expected an orphan-heading error, got: {err}"
        );

        // An H4 top-level section (double skip) is likewise rejected.
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n#### A\n\na\n";
        assert!(
            Prompt::parse(src, "test", &NullObserver).is_err(),
            "an H4 top-level section must be rejected"
        );
    }

    #[test]
    fn unknown_frontmatter_field_is_rejected() {
        let src = "---\nname: x\ndescription: d\nnot_a_real_field: 1\n---\n\n# T\n\n## S\n\np\n";
        let err = Prompt::parse(src, "test", &NullObserver)
            .expect_err("an unknown frontmatter field must be rejected");
        assert!(
            err.to_string().contains("not_a_real_field")
                || err.to_string().contains("unknown field"),
            "expected an unknown-field error, got: {err}"
        );
        // A known-field-only frontmatter still parses.
        let ok = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
        assert!(Prompt::parse(ok, "test", &NullObserver).is_ok());
    }

    #[test]
    fn empty_section_heading_is_rejected() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## \n\na\n";
        let err = Prompt::parse(src, "test", &NullObserver)
            .expect_err("an empty section heading must be rejected");
        assert!(
            err.to_string().contains("must not be empty"),
            "expected an empty-heading error, got: {err}"
        );
    }

    #[test]
    fn duplicate_sibling_section_names_are_rejected() {
        // Two H2 siblings named `S` are ambiguous section targets.
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\na\n\n## S\n\nb\n";
        let err = Prompt::parse(src, "test", &NullObserver)
            .expect_err("duplicate sibling section names must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("duplicate sibling section name"),
            "expected a duplicate-sibling error, got: {err}"
        );
        // PF-PARSER-002: both duplicate locations are named. The first `## S` is
        // at body line 3 (line 8 overall) and the second at body line 7 (line 12).
        assert!(
            message.contains("first declared at line 8") && message.contains("again at line 12"),
            "both duplicate locations must be reported, got: {message}"
        );
        // PF-PARSER-008: a structured parse error carries a stable kind and a
        // byte span rather than inferring them from the message.
        assert_eq!(err.kind(), ParseErrorKind::Structure);
        let (start, end) = err.span().expect("duplicate section carries a span");
        assert!(
            start < end,
            "span must be a non-empty range, got {start}..{end}"
        );

        // The same name under DIFFERENT parents (not siblings) is allowed.
        let ok = "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n### S\n\nx\n\n## B\n\nb\n\n### S\n\ny\n";
        assert!(
            Prompt::parse(ok, "test", &NullObserver).is_ok(),
            "the same name under different parents is not a sibling collision"
        );
    }

    #[test]
    fn max_tool_iterations_parses_positive_and_defaults_when_absent() {
        let declared =
            "---\nname: x\ndescription: d\nmax_tool_iterations: 20\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(declared, "test", &NullObserver).unwrap();
        assert_eq!(
            p.frontmatter.max_tool_iterations,
            MaxToolIterations::Limit(std::num::NonZeroU32::new(20).unwrap())
        );

        let absent = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(absent, "test", &NullObserver).unwrap();
        assert_eq!(
            p.frontmatter.max_tool_iterations,
            MaxToolIterations::Default
        );
    }

    #[test]
    fn max_tool_iterations_rejects_zero_negative_and_overflow() {
        let body = |value: &str| {
            format!(
                "---\nname: x\ndescription: d\nmax_tool_iterations: {value}\n---\n\n# T\n\n## S\n\np\n"
            )
        };
        for bad in ["0", "-1", "1001", "100000000000"] {
            let error = Prompt::parse(&body(bad), "test", &NullObserver)
                .expect_err(&format!("max_tool_iterations {bad} must be rejected"));
            assert_eq!(
                error.kind(),
                ParseErrorKind::Frontmatter,
                "value {bad}: {error}"
            );
        }
    }

    #[test]
    fn max_tool_iterations_accepts_the_upper_boundary() {
        let body = format!(
            "---\nname: x\ndescription: d\nmax_tool_iterations: {MAX_TOOL_ITERATIONS}\n---\n\n# T\n\n## S\n\np\n"
        );
        let p = Prompt::parse(&body, "test", &NullObserver).unwrap();
        assert_eq!(
            p.frontmatter.max_tool_iterations,
            MaxToolIterations::Limit(std::num::NonZeroU32::new(MAX_TOOL_ITERATIONS).unwrap())
        );
    }

    #[test]
    fn max_tool_iterations_resolve_uses_default_only_when_absent() {
        assert_eq!(MaxToolIterations::Default.resolve(24), 24);
        assert_eq!(
            MaxToolIterations::Limit(std::num::NonZeroU32::new(3).unwrap()).resolve(24),
            3
        );
    }

    #[test]
    fn first_h2_is_entry_regardless_of_name() {
        let src =
            "---\nname: x\ndescription: d\n---\n\n# T\n\n## Zebra\n\nfirst\n\n## Main\n\nsecond\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        assert_eq!(p.entry().name, "Zebra");
    }

    #[test]
    fn detection_reads_promptforge_major() {
        let src = "---\nname: x\ndescription: d\npromptforge: 1\n---\n\n## S\n\np\n";
        assert_eq!(promptforge_version(src), Some(1));
    }

    #[test]
    fn detection_needs_only_the_promptforge_key() {
        // No name or description, but the key is present.
        let src = "---\npromptforge: 2\n---\n\n## S\n\np\n";
        assert_eq!(promptforge_version(src), Some(2));
    }

    #[test]
    fn detection_absent_key_is_none() {
        let src = "---\nname: x\ndescription: d\n---\n\n## S\n\np\n";
        assert_eq!(promptforge_version(src), None);
    }

    #[test]
    fn detection_no_frontmatter_is_none() {
        let src = "# Just a title\n\nPlain prose with no frontmatter block at all.\n";
        assert_eq!(promptforge_version(src), None);
    }

    #[test]
    fn detection_malformed_frontmatter_is_none() {
        // Opening delimiter but never closed.
        let unclosed = "---\npromptforge: 1\nname: x\n\n## S\n\np\n";
        assert_eq!(promptforge_version(unclosed), None);

        // Closed, but not valid YAML.
        let bad_yaml = "---\npromptforge: 1\n  : : oops\n---\n\n## S\n\np\n";
        assert_eq!(promptforge_version(bad_yaml), None);
    }

    #[test]
    fn frontmatter_exposes_promptforge_field() {
        let with = "---\nname: x\ndescription: d\npromptforge: 1\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(with, "test", &NullObserver).unwrap();
        assert_eq!(p.frontmatter.promptforge, Some(1));

        let without = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(without, "test", &NullObserver).unwrap();
        assert_eq!(p.frontmatter.promptforge, None);
    }

    #[test]
    fn bullet_parser_strips_unordered_markers() {
        let items = parse_bullet_items("- alpha\n* beta\n- gamma", "test").unwrap();
        assert_eq!(items, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn bullet_parser_strips_ordered_markers() {
        let items = parse_bullet_items("1. first\n2. second\n3) third", "test").unwrap();
        assert_eq!(items, vec!["first", "second", "third"]);
    }

    #[test]
    fn bullet_parser_ignores_blank_lines() {
        let items = parse_bullet_items("- alpha\n\n- beta\n  \n- gamma", "test").unwrap();
        assert_eq!(items, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn bullet_parser_rejects_non_list_content() {
        let err = parse_bullet_items("- alpha\nnot a bullet\n- gamma", "test")
            .expect_err("non-list content must error");
        assert!(
            err.to_string().contains("non-list content"),
            "error was: {err}"
        );
    }

    #[test]
    fn bullet_parser_rejects_empty_list() {
        let err = parse_bullet_items("", "test").expect_err("empty list must error");
        assert!(err.to_string().contains("no items"), "error was: {err}");
    }

    #[test]
    fn bullet_parser_rejects_empty_item() {
        let err =
            parse_bullet_items("- alpha\n- \n- gamma", "test").expect_err("empty item must error");
        assert!(
            err.to_string().contains("empty bullet item"),
            "error was: {err}"
        );
    }

    #[test]
    fn list_h3_parses_items_at_load_time() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Parent\n\np\n\n### Items\n\n- alpha\n- beta\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        let items_section = &p.sections[0].children[0];
        assert_eq!(items_section.name, "Items");
        assert_eq!(items_section.items, vec!["alpha", "beta"]);
    }

    #[test]
    fn non_list_h3_has_empty_items() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Parent\n\np\n\n### Worker\n\n```lua\nreturn item\n```\n\nDo work on {{ item }}.\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        let worker = &p.sections[0].children[0];
        assert_eq!(worker.name, "Worker");
        assert!(worker.items.is_empty());
    }

    #[test]
    fn epilog_source_line_maps_runtime_error_to_absolute_line() {
        // Lines:
        //  1: ---
        //  2: name: x
        //  3: description: d
        //  4: ---
        //  5: (empty)
        //  6: # T
        //  7: (empty)
        //  8: ## Check
        //  9: (empty)
        // 10: Ask the model.
        // 11: (empty)
        // 12: ```lua       <- epilog opens
        // 13: local a = 1  <- epilog line 1 (source_line = 13)
        // 14: assert(false) <- epilog line 2 (absolute = 14)
        // 15: ```
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Check\n\nAsk the model.\n\n```lua\nlocal a = 1\nassert(false)\n```\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
        let epilog = prompt.entry().epilog().expect("epilog must exist");

        assert_eq!(
            epilog.source_line().get(),
            13,
            "epilog Lua starts on line 13"
        );
        assert_eq!(epilog.source(), "local a = 1\nassert(false)");

        // Simulate a runtime error: assert(false) is on chunk line 2.
        // Absolute line = 13 + 2 - 1 = 14.
        let lua = mlua::Lua::new();
        let function = epilog.load(&lua).expect("bytecode must load");
        let raw_error = function
            .call::<()>(())
            .expect_err("assert(false) must fail");
        let mapped = epilog.map_runtime_error(&raw_error);
        let msg = mapped.to_string();
        assert!(
            msg.contains(":14:"),
            "error must contain absolute line 14: {msg}"
        );
    }

    #[test]
    fn prologue_source_line_maps_correctly() {
        // Lines:
        //  1: ---
        //  2: name: x
        //  3: description: d
        //  4: ---
        //  5: (empty)
        //  6: # T
        //  7: (empty)
        //  8: ## Work
        //  9: (empty)
        // 10: ```lua       <- prologue opens
        // 11: assert(false) <- prologue line 1 (source_line = 11, absolute = 11)
        // 12: ```
        // 13: (empty)
        // 14: Do the work.
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Work\n\n```lua\nassert(false)\n```\n\nDo the work.\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
        let prologue = prompt.entry().prologue().expect("prologue must exist");

        assert_eq!(
            prologue.source_line().get(),
            11,
            "prologue Lua starts on line 11"
        );

        let lua = mlua::Lua::new();
        let function = prologue.load(&lua).expect("bytecode must load");
        let raw_error = function
            .call::<()>(())
            .expect_err("assert(false) must fail");
        let mapped = prologue.map_runtime_error(&raw_error);
        let msg = mapped.to_string();
        assert!(
            msg.contains(":11:"),
            "error must contain absolute line 11: {msg}"
        );
    }

    #[test]
    fn multi_line_chunk_maps_inner_line_correctly() {
        // Epilog with assert on line 2 of the fence.
        //  1-4: frontmatter
        //  5: empty
        //  6: # T
        //  7: empty
        //  8: ## S
        //  9: empty
        // 10: Prose.
        // 11: empty
        // 12: ```lua
        // 13: local x = 1    <- source_line = 13
        // 14: local y = 2
        // 15: assert(false)  <- chunk line 3, absolute = 13 + 3 - 1 = 15
        // 16: ```
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nProse.\n\n```lua\nlocal x = 1\nlocal y = 2\nassert(false)\n```\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
        let epilog = prompt.entry().epilog().expect("epilog must exist");

        assert_eq!(epilog.source_line().get(), 13);

        let lua = mlua::Lua::new();
        let function = epilog.load(&lua).expect("bytecode must load");
        let raw_error = function
            .call::<()>(())
            .expect_err("assert(false) must fail");
        let mapped = epilog.map_runtime_error(&raw_error);
        let msg = mapped.to_string();
        assert!(
            msg.contains(":15:"),
            "error must contain absolute line 15: {msg}"
        );
    }

    #[test]
    fn shared_library_source_line_is_correct() {
        // Lines:
        //  1: ---
        //  2: name: x
        //  3: description: d
        //  4: ---
        //  5: (empty)
        //  6: # T
        //  7: (empty)
        //  8: ```lua shared <- shared opens
        //  9: function f()  <- source_line = 9
        // 10: end
        // 11: ```
        // 12: (empty)
        // 13: ## S
        // 14: (empty)
        // 15: p
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nfunction f()\nend\n```\n\n## S\n\np\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
        let replay = prompt.replay.as_ref().expect("replay must exist");
        assert_eq!(replay.source_line().get(), 9, "shared Lua starts on line 9");
    }
}
