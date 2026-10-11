//! Frontmatter parsing and heading-tree construction: the [`Frontmatter`]
//! model, the `max_tool_iterations` cap, frontmatter splitting/version
//! detection, and the markdown heading walk that builds the [`Section`]
//! tree.

use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use promptforge_types::emitter::Emitter;

use super::fence::{RawBlock, lua_block_location, split_section_blocks, trim_leading_blank_lines};
use super::list::{is_all_list_markers, parse_bullet_items};
use super::{Block, BodyText, LuaProgram, ParseErrorKind, Section};
use crate::{Error, Result};

#[path = "build-frontmatter.rs"]
mod frontmatter;

pub(crate) use frontmatter::split_frontmatter;
pub use frontmatter::{
    FileDecl, Frontmatter, MAX_TOOL_ITERATIONS, MaxToolIterations, promptforge_version,
};

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

/// Locates `part` inside the shared `text` it was sliced from.
///
/// # Errors
/// Returns [`Error::Internal`] when `part` is not a slice of `text`.
pub(crate) fn body_text(text: &Arc<str>, part: &str) -> Result<BodyText> {
    BodyText::within(text, part).ok_or(Error::Internal(
        "parser: section text is not a slice of the prompt body",
    ))
}

/// A heading with its title and the prose/Lua that follows it (before the next
/// heading of any level).
#[derive(Debug, Clone)]
pub(crate) struct Heading<'a> {
    pub(crate) level: u8,
    pub(crate) title: String,
    pub(crate) content: &'a str,
    /// 1-based line number within `body` where `content` begins.
    pub(crate) content_start_line: u32,
    /// 1-based line number within `body` of the heading line itself.
    source_line: u32,
    /// Byte range of the heading within `body`, kept for span diagnostics.
    span: Range<usize>,
}

/// Counts the number of `\n` characters in `text[..byte_offset]`.
///
/// # Errors
/// Returns [`Error::Internal`] when `byte_offset` is out of bounds or not a
/// UTF-8 boundary, or when the newline count exceeds `u32`.
pub(crate) fn newlines_before(text: &str, byte_offset: usize) -> Result<u32> {
    let prefix = text.get(..byte_offset).ok_or(Error::Internal(
        "parser: source byte offset invariant broken",
    ))?;
    u32::try_from(prefix.matches('\n').count())
        .map_err(|_| Error::Internal("parser: newline count exceeded u32 range"))
}

/// Converts a `HeadingLevel` to its numeric level.
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

/// Walks the markdown body and collects every heading with the content that
/// follows it, up to the next heading of any level.
///
/// A heading inside a block quote or list item is not a section boundary; its
/// text stays in the enclosing heading's content.
pub(crate) fn collect_headings(body: &str) -> Result<Vec<Heading<'_>>> {
    // First pass: find each heading's level, title, and source byte range.
    struct Raw {
        level: u8,
        title: String,
        range: Range<usize>,
    }
    let mut raws: Vec<Raw> = Vec::new();
    let mut current: Option<(u8, Range<usize>, String)> = None;
    let mut container_depth: usize = 0;

    for (event, range) in Parser::new_ext(body, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::BlockQuote(_) | Tag::Item) => container_depth += 1,
            Event::End(TagEnd::BlockQuote(_) | TagEnd::Item) => {
                container_depth = container_depth.saturating_sub(1);
            }
            Event::Start(Tag::Heading { level, .. }) if container_depth == 0 => {
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
            content,
            content_start_line,
            source_line,
            span: raws[i].range.clone(),
        });
    }
    Ok(headings)
}

/// Builds one heading's executable blocks while preserving source positions.
/// Prose reads in place from `body`, the shared text `heading` was sliced from.
fn build_heading_blocks(
    body: &Arc<str>,
    heading: &Heading<'_>,
    name: &str,
    frontmatter_lines: u32,
    emitter: &Emitter,
) -> Result<Vec<Block>> {
    let content_abs_line = line_add(frontmatter_lines, heading.content_start_line)?;
    let raw_blocks = split_section_blocks(heading.content, name)?;
    let has_prose = raw_blocks
        .iter()
        .any(|block| matches!(block, RawBlock::Prose(_)));
    let total = raw_blocks.len();
    let mut blocks = Vec::with_capacity(total);
    for (index, raw) in raw_blocks.into_iter().enumerate() {
        match raw {
            RawBlock::Prose(text) => blocks.push(Block::Prose {
                text: body_text(body, text)?,
            }),
            RawBlock::Lua {
                source,
                line_offset,
            } => {
                let abs_line = line_add(content_abs_line, line_offset)?;
                let location = lua_block_location(name, index, total, has_prose);
                let program = LuaProgram::compile(
                    &source,
                    &location,
                    nz_source_line(abs_line)?,
                    emitter,
                    name,
                )?;
                blocks.push(Block::Lua(program));
            }
        }
    }
    Ok(blocks)
}

/// Rejects an orphan deep heading: one that skips a level (e.g. an H4 under an
/// H2 with no intervening H3, or an H3/H4 top-level section with no parent H2).
/// Such a heading has no well-defined parent, so reject it rather than
/// silently reparenting it to a shallower ancestor.
fn check_child_level(heading: &Heading<'_>, parent_level: u8) -> Result<()> {
    let level = heading.level;
    if level > parent_level + 1 {
        return Err(Error::parse_at(
            ParseErrorKind::Structure,
            heading.span.clone(),
            format!(
                "section `{}` is an orphan H{level} heading with no parent H{}",
                heading.title.trim(),
                parent_level + 1
            ),
        ));
    }
    Ok(())
}

/// Returns the heading's section name. A section's name is its runtime address
/// (jumps, lookups, fanout), so an empty/whitespace heading is unaddressable
/// and must be rejected at parse.
fn section_name(heading: &Heading<'_>, level: u8) -> Result<String> {
    let name = heading.title.clone();
    if name.trim().is_empty() {
        return Err(Error::parse_at(
            ParseErrorKind::Structure,
            heading.span.clone(),
            format!("an H{level} section heading must not be empty"),
        ));
    }
    Ok(name)
}

/// Returns the section's list items, or none when it is not a list. A section
/// is a list only when it has no Lua and every nonblank prose line is a list
/// marker; one incidental bullet in mixed prose leaves a non-marker line, so
/// the section stays prose.
fn list_items(body: &Arc<str>, blocks: &[Block], name: &str) -> Result<Vec<BodyText>> {
    let has_no_lua = blocks
        .iter()
        .all(|block| matches!(block, Block::Prose { .. }));
    let prose_for_items = blocks.iter().find_map(|block| match block {
        Block::Prose { text, .. } => Some(text.as_str()),
        Block::Lua(_) => None,
    });
    if has_no_lua
        && let Some(prose) = prose_for_items
        && is_all_list_markers(prose)
    {
        parse_bullet_items(prose, name)?
            .into_iter()
            .map(|item| body_text(body, item))
            .collect()
    } else {
        Ok(Vec::new())
    }
}

/// Rejects a section whose name an earlier sibling already took. Sections are
/// addressed by name (jumps, lookups), so two siblings sharing a name would
/// make the target ambiguous. The error names BOTH heading locations so the
/// author can find each one.
fn check_unique_sibling(
    siblings: &[(String, u32)],
    name: &str,
    heading: &Heading<'_>,
    heading_abs_line: u32,
) -> Result<()> {
    if let Some((_, first_line)) = siblings.iter().find(|(n, _)| n == name) {
        return Err(Error::parse_at(
            ParseErrorKind::Structure,
            heading.span.clone(),
            format!(
                "duplicate sibling section name `{name}`: first declared at line {first_line}, again at line {heading_abs_line}; sibling section names must be unique"
            ),
        ));
    }
    Ok(())
}

/// Builds a section tree from a flat, document-ordered list of headings.
///
/// Recursion consumes headings whose level is deeper than `parent_level`; a
/// heading at or above `parent_level` belongs to an ancestor and stops the
/// current level. A heading that skips a level (an orphan deep heading, such as
/// an H4 directly under an H2, or an H3 top-level section with no parent H2) is
/// rejected: every heading must be exactly one level deeper than its parent.
///
/// # Errors
/// Returns [`Error::ParseStructured`] for an orphan heading, an empty heading,
/// duplicate sibling names, or malformed section content. Propagates
/// [`Error::LuaCompile`] or [`Error::Lua`] from Lua compilation and
/// [`Error::Internal`] from invalid or overflowing source-line calculations,
/// or from section text that is not a slice of `body`.
pub(crate) fn build_sections(
    body: &Arc<str>,
    headings: &[Heading<'_>],
    pos: &mut usize,
    parent_level: u8,
    frontmatter_lines: u32,
    emitter: &Emitter,
) -> Result<Vec<Section>> {
    let mut result = Vec::new();
    // Parallel to `result`: each sibling's name and its 1-based heading line, so
    // a duplicate can name both the first and the offending location.
    let mut sibling_lines: Vec<(String, u32)> = Vec::new();
    while *pos < headings.len() {
        let h = &headings[*pos];
        let level = h.level;
        if level <= parent_level {
            break;
        }
        check_child_level(h, parent_level)?;
        let name = section_name(h, level)?;
        let heading_abs_line = line_add(frontmatter_lines, h.source_line)?;
        let blocks = build_heading_blocks(body, h, &name, frontmatter_lines, emitter)?;
        *pos += 1;
        let children = build_sections(body, headings, pos, level, frontmatter_lines, emitter)?;
        let items = list_items(body, &blocks, &name)?;
        check_unique_sibling(&sibling_lines, &name, h, heading_abs_line)?;
        sibling_lines.push((name.clone(), heading_abs_line));

        result.push(Section {
            name,
            level,
            blocks,
            children,
            items,
            markdown: body_text(body, trim_leading_blank_lines(h.content).trim_end())?,
        });
    }
    Ok(result)
}

#[cfg(test)]
#[path = "build-tests.rs"]
mod position_tests;
