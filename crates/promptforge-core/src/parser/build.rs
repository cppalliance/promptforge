//! Frontmatter parsing and heading-tree construction (PF-PARSER-012).
//!
//! Split out of the `parser` facade so the facade (public types +
//! orchestration) stays small. This owns the [`Frontmatter`] model, the
//! `max_tool_iterations` cap, frontmatter splitting/version detection, and the
//! markdown heading walk that builds the [`Section`] tree.

use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::fence::{RawBlock, lua_block_location, split_section_blocks};
use super::list::{is_all_list_markers, parse_bullet_items};
use super::{Block, ParseErrorKind, Section};
use crate::lua::LuaProgram;
use crate::observe::Observer;
use crate::{Error, Result};

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
pub(crate) fn nz_source_line(line: u32) -> Result<std::num::NonZeroU32> {
    std::num::NonZeroU32::new(line).ok_or(Error::Internal(
        "parser: computed 1-based source line was zero",
    ))
}

/// Adds two 1-based line components with overflow checking.
///
/// # Errors
/// Returns [`Error::Internal`] when the sum overflows `u32`, rather than
/// saturating and hiding a broken position.
pub(crate) fn line_add(a: u32, b: u32) -> Result<u32> {
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
/// A heading with its title and the prose/Lua that follows it (before the next
/// heading of any level).
#[derive(Debug, Clone)]
pub(crate) struct Heading {
    pub(crate) level: u8,
    pub(crate) title: String,
    pub(crate) content: String,
    /// 1-based line number within `body` where `content` begins.
    pub(crate) content_start_line: u32,
    /// 1-based line number within `body` of the heading line itself.
    pub(crate) source_line: u32,
    /// Byte range of the heading within `body`, carried for span diagnostics.
    pub(crate) span: Range<usize>,
}

/// Split a file into its YAML frontmatter, its markdown body, and the
/// number of lines consumed by the frontmatter block (both `---` delimiters
/// and everything between them).
///
/// The file must open with a `---` line and close the frontmatter with another
/// `---` line. `str::lines` handles both `\n` and `\r\n`.
pub(crate) fn split_frontmatter(input: &str) -> Result<(String, String, u32)> {
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
    let probe: Probe = serde_yaml_ng::from_str(&yaml).ok()?;
    probe.promptforge
}

/// Counts the number of `\n` characters in `text[..byte_offset]`.
///
/// # Errors
/// Returns [`Error::Internal`] when the newline count exceeds `u32`, rather than
/// saturating to `u32::MAX` and hiding an implausibly large source position.
pub(crate) fn newlines_before(text: &str, byte_offset: usize) -> Result<u32> {
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
pub(crate) fn collect_headings(body: &str) -> Result<Vec<Heading>> {
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
pub(crate) fn build_sections(
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
