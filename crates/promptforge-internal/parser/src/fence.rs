//! Checks the strict `lua` fence rule and splits content into Lua and prose blocks.
//!
//! H1 and section content is an alternating sequence of `lua` fences and
//! prose. A `lua` fence opens with a line that is exactly three backticks
//! then `lua` and closes with a line that is exactly three backticks. This
//! module finds the first fence in the body that breaks the Label, Opener, or
//! Closer rule, or the first Lookalike line, so the parse can reject it, and
//! splits content into [`RawBlock`]s that the facade compiles into
//! [`Block`]s. Lines inside the body of another fenced code block are never
//! inspected, which is how a document shows a Lua fence as an example.

use std::ops::Range;
use std::sync::Arc;

use promptforge_types::emitter::Emitter;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag};

use crate::{Block, Error, LuaProgram, ParseErrorKind, Result, sections};

/// Uncompiled block produced while scanning section content.
pub(super) enum RawBlock<'a> {
    Lua {
        source: String,
        /// Lines before the first line of Lua source within section content.
        line_offset: u32,
    },
    Prose(&'a str),
}

/// Compiles H1 content as alternating live Lua and prose blocks.
///
/// `content` is the H1's stretch of `body`, and `content_abs_line` is the
/// 1-based line number in the full input where `content` begins.
///
/// # Errors
/// Returns a fence-classified parse error for an unclosed fence, and a Lua
/// compilation error for invalid Lua.
pub(super) fn split_h1(
    body: &Arc<str>,
    content: &str,
    title: &str,
    content_abs_line: u32,
    emitter: &Emitter,
) -> Result<(Vec<Block>, String)> {
    let raw_blocks = split_section_blocks(content, title)?;
    let mut blocks = Vec::with_capacity(raw_blocks.len());
    for raw in raw_blocks {
        match raw {
            RawBlock::Prose(text) => blocks.push(Block::Prose {
                text: sections::body_text(body, text)?,
            }),
            RawBlock::Lua {
                source,
                line_offset,
            } => {
                let location = format!("H1 `{title}` lua");
                let line = sections::line_add(content_abs_line, line_offset)?;
                blocks.push(Block::Lua(LuaProgram::compile(
                    &source,
                    &location,
                    sections::nz_source_line(line)?,
                    emitter,
                    title,
                )?));
            }
        }
    }
    if matches!(
        blocks.as_slice(),
        [Block::Prose { text }] if text.as_str().is_empty()
    ) {
        blocks.clear();
    }
    let description_text = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Prose { text, .. } if !text.as_str().is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok((blocks, description_text))
}

/// Removes complete leading lines that contain only whitespace.
pub(crate) fn trim_leading_blank_lines(content: &str) -> &str {
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        if line.trim().is_empty() {
            offset += line.len();
        } else {
            break;
        }
    }
    &content[offset..]
}

/// Extracts source through the first exact, unindented triple-backtick closing
/// line.
fn extract_exact_fence<'a>(content: &'a str, label: &str) -> Result<(String, &'a str)> {
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        let text = line.strip_suffix('\n').unwrap_or(line);
        let text = text.strip_suffix('\r').unwrap_or(text);
        if text == "```" {
            let source = content[..offset].trim_end_matches(['\r', '\n']).to_string();
            return Ok((source, &content[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err(Error::parse(
        ParseErrorKind::Fence,
        format!("{label} fence is not closed"),
    ))
}

/// Removes one exact unindented `lua` opening line from `content`.
fn strip_exact_lua_opening(content: &str) -> Option<&str> {
    content
        .strip_prefix("```lua\r\n")
        .or_else(|| content.strip_prefix("```lua\n"))
        .or_else(|| (content == "```lua").then_some(""))
}

/// Yields the extent and the label of every fence the Markdown parser
/// reports, in source order.
///
/// Each extent starts at the opening line's first byte and ends where the
/// parser ends the block. That includes fences inside block quotes and list
/// items, and never a fence inside another code block's body, so fence-looking
/// lines in a longer fence stay prose rather than being reserved or rejected.
fn fence_openings(content: &str) -> impl Iterator<Item = (Range<usize>, CowStr<'_>)> {
    Parser::new_ext(content, Options::empty())
        .into_offset_iter()
        .filter_map(|(event, range)| {
            let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(label))) = event else {
                return None;
            };
            let line_start = content[..range.start]
                .rfind('\n')
                .map_or(0, |newline| newline + 1);
            Some((line_start..range.end, label))
        })
}

/// Returns the byte offsets of exact `lua` opening lines.
fn exact_lua_openings(content: &str) -> Vec<usize> {
    fence_openings(content)
        .filter_map(|(extent, _)| {
            (content[extent.start..].lines().next() == Some("```lua")).then_some(extent.start)
        })
        .collect()
}

/// The rule a fence or line breaks, as found by [`first_fence_violation`].
pub(super) enum FenceViolation {
    /// Label: the fence's label starts with `lua` in any letter case without
    /// being exactly `lua`.
    Label(String),
    /// Opener: a fence labeled exactly `lua` whose opening line is not exactly
    /// three backticks then `lua`, or a Lookalike line.
    Opener,
    /// Closer: a fence whose opening line is exact and whose closing line is
    /// not exactly three backticks.
    Closer,
}

/// Returns the line's byte range, without its terminator, and the rule
/// broken by the first violation in `body` in source order.
///
/// Each fence the Markdown parser reports is checked against the Label rule,
/// then the Opener rule, then the Closer rule. A fence the parser leaves
/// unclosed reports nothing here; [`split_section_blocks`] reports it. Every
/// other line is checked against the Lookalike rule: after leading spaces and
/// tabs, three or more backticks or tildes, optional spaces or tabs, then
/// `lua` in any letter case. Such a line sits where the parser reports no
/// fence, such as in an indented code block or a raw HTML block, and breaks
/// the Opener rule. Lines inside a fence's extent are never checked against
/// the Lookalike rule, so a fence nested in another fence's body never fails.
pub(super) fn first_fence_violation(body: &str) -> Option<(Range<usize>, FenceViolation)> {
    let mut extents = Vec::new();
    let mut fence = None;
    for (extent, label) in fence_openings(body) {
        fence = fence.or_else(|| fence_violation(body, extent.clone(), &label));
        extents.push(extent);
    }
    let lookalike = body
        .split_inclusive('\n')
        .scan(0, |next, line| {
            let start = *next;
            *next += line.len();
            Some(start)
        })
        .filter(|start| !extents.iter().any(|extent| extent.contains(start)))
        .map(|start| line_at(body, start))
        .find(|line| is_lua_lookalike(&body[line.clone()]))
        .map(|line| (line, FenceViolation::Opener));
    [fence, lookalike]
        .into_iter()
        .flatten()
        .min_by_key(|(line, _)| line.start)
}

/// Checks one reported fence against the Label, Opener, and Closer rules, in
/// that order.
fn fence_violation(
    body: &str,
    extent: Range<usize>,
    label: &str,
) -> Option<(Range<usize>, FenceViolation)> {
    let opening = line_at(body, extent.start);
    if label != "lua" {
        return is_lua_like(label).then(|| (opening, FenceViolation::Label(label.to_owned())));
    }
    if &body[opening.clone()] != "```lua" {
        return Some((opening, FenceViolation::Opener));
    }
    let closing = closing_line(body, extent)?;
    (&body[closing.clone()] != "```").then_some((closing, FenceViolation::Closer))
}

/// Byte range of the line starting at `start`, without its terminator.
fn line_at(content: &str, start: usize) -> Range<usize> {
    let rest = &content[start..];
    let line = rest.find('\n').map_or(rest, |end| &rest[..end]);
    let line = line.strip_suffix('\r').unwrap_or(line);
    start..start + line.len()
}

/// Byte range of the closing line of an exact `lua` fence whose block covers
/// `extent`, or `None` when the block is unclosed.
///
/// The closing line is the block's own last line, so this always agrees
/// with the Markdown parser on where the fence ends. A block the parser
/// leaves unclosed ends on a body line, which never has a closing line's
/// shape, because such a line would have closed it.
fn closing_line(body: &str, extent: Range<usize>) -> Option<Range<usize>> {
    let block = body[extent.clone()].trim_end_matches(['\r', '\n']);
    let last = block.rfind('\n')? + 1;
    let line = line_at(body, extent.start + last);
    closes_three_backtick_fence(&body[line.clone()]).then_some(line)
}

/// Whether `line` has the shape of a line that closes a fence opened by three
/// backticks: at most three spaces of indent, three or more backticks, then
/// only spaces or tabs.
fn closes_three_backtick_fence(line: &str) -> bool {
    let unindented = line.trim_start_matches(' ');
    let marker = unindented.trim_start_matches('`');
    line.len() - unindented.len() <= 3
        && unindented.len() - marker.len() >= 3
        && marker.trim_matches([' ', '\t']).is_empty()
}

/// Whether `line` breaks the Lookalike rule.
fn is_lua_lookalike(line: &str) -> bool {
    let text = line.trim_start_matches([' ', '\t']);
    let Some(marker) = text.chars().next().filter(|c| matches!(c, '`' | '~')) else {
        return false;
    };
    let after_marker = text.trim_start_matches(marker);
    text.len() - after_marker.len() >= 3
        && is_lua_like(after_marker.trim_start_matches([' ', '\t']))
}

/// Whether `text` starts with `lua` in any letter case.
fn is_lua_like(text: &str) -> bool {
    text.get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("lua"))
}

/// Byte offset where leading blank lines end.
fn leading_content_start(content: &str) -> usize {
    content.len() - trim_leading_blank_lines(content).len()
}

/// Byte ranges of every genuine thematic break in `content`.
///
/// Pulldown reports a `Rule` only for a real CommonMark thematic break: a
/// `---` inside a fenced code block is code, and a text line immediately
/// followed by `---` is a setext heading underline, never a rule.
fn rule_ranges(content: &str) -> Vec<Range<usize>> {
    Parser::new_ext(content, Options::empty())
        .into_offset_iter()
        .filter_map(|(event, range)| matches!(event, Event::Rule).then_some(range))
        .collect()
}

/// Returns the pending Markdown of one prose segment: everything after the
/// segment's last thematic break.
///
/// The break itself is never included, and commentary above it is excluded.
fn pending_prose<'a>(content: &'a str, segment: Range<usize>, rules: &[Range<usize>]) -> &'a str {
    let start = rules
        .iter()
        .filter(|rule| rule.start >= segment.start && rule.end <= segment.end)
        .map(|rule| rule.end)
        .max()
        .unwrap_or(segment.start);
    &content[start..segment.end]
}

/// Splits a section into alternating exact `lua` fences and prose segments.
///
/// Every exact `lua` fence the Markdown parser reports becomes a Lua block.
/// Text between fences becomes prose (including an empty segment between
/// consecutive fences, so classic prologue/epilog with empty prose stays
/// `[Lua, Prose, Lua]`). Leading blank lines before a leading fence are
/// discarded. Each prose segment is the pending Markdown buffer for the
/// fence that follows it: a thematic break resets the buffer, so only the
/// Markdown below the segment's last break is captured.
///
/// The parse runs [`first_fence_violation`] on the whole body first, so the
/// first exact closing line after an exact opener is the line the Markdown
/// parser closes the fence on.
///
/// # Errors
/// Returns a fence-classified parse error when an exact `lua` fence is not
/// closed, and [`Error::Internal`] when an opening the Markdown parser
/// reports falls inside the previous fence's extracted source.
pub(super) fn split_section_blocks<'a>(
    content: &'a str,
    section: &str,
) -> Result<Vec<RawBlock<'a>>> {
    let openings = exact_lua_openings(content);
    let rules = rule_ranges(content);
    if openings.is_empty() {
        let pending = pending_prose(content, 0..content.len(), &rules);
        return Ok(vec![RawBlock::Prose(pending.trim())]);
    }

    let leading_start = leading_content_start(content);
    let mut blocks = Vec::new();
    let mut pos = 0usize;

    for (index, &opening) in openings.iter().enumerate() {
        let after_open = if opening < pos {
            None
        } else {
            strip_exact_lua_opening(&content[opening..])
        };
        let Some(after_open) = after_open else {
            return Err(Error::Internal(
                "parser: section fence split disagrees with the Markdown parser",
            ));
        };
        // Preserve classic location wording for unclosed leading/trailing fences.
        let label = if index == 0 && opening == leading_start {
            format!("section `{section}` prologue `lua`")
        } else if index + 1 == openings.len() {
            format!("section `{section}` epilog `lua`")
        } else {
            format!("section `{section}` `lua`")
        };
        let (source, rest) = extract_exact_fence(after_open, &label)?;
        let fence_end = content.len() - rest.len();

        if !(pos == 0 && opening == leading_start) {
            let pending = pending_prose(content, pos..opening, &rules);
            blocks.push(RawBlock::Prose(pending.trim()));
        }

        let line_offset = sections::line_add(sections::newlines_before(content, opening)?, 1)?;
        blocks.push(RawBlock::Lua {
            source,
            line_offset,
        });
        pos = fence_end;
    }

    let trailing = pending_prose(content, pos..content.len(), &rules).trim();
    if !trailing.is_empty() {
        blocks.push(RawBlock::Prose(trailing));
    }

    Ok(blocks)
}

/// Compile label for one Lua block of the section named by `label`,
/// preserving classic prologue/epilog names.
///
/// Every other block is named by `ordinal`, its 1-based place among the
/// section's Lua blocks. The label is the display name mapped runtime
/// errors give the block; mapping itself matches the block's line-based
/// chunk name, so the label need not be unique or short.
pub(super) fn lua_block_location(
    label: &str,
    index: usize,
    total: usize,
    has_prose: bool,
    ordinal: usize,
) -> String {
    let is_first = index == 0;
    let is_last = index + 1 == total;
    if is_first {
        return format!("{label} prologue");
    }
    if is_last && has_prose {
        return format!("{label} epilog");
    }
    format!("{label} lua {ordinal}")
}
