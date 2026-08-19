//! Exact `lua` / `lua shared` fence recognition and block splitting.
//!
//! H1 and section content is an alternating sequence of exact, unindented
//! triple-backtick `lua` fences and prose. This module recognizes those fences
//! (never near-misses or fences nested inside longer code blocks), extracts the
//! optional `lua shared` H1 library, and splits content into [`RawBlock`]s that
//! the facade compiles into [`Block`]s.

use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};

use super::Block;
use super::build::{line_add, newlines_before, nz_source_line};
use crate::lua::LuaProgram;
use crate::observe::Observer;
use crate::{Error, Result};

/// Uncompiled block produced while scanning section content.
pub(super) enum RawBlock {
    Lua {
        source: String,
        /// Lines before the first line of Lua source within section content.
        line_offset: u32,
    },
    Prose(String),
}

/// Extracts the optional exact `lua shared` library from H1 and compiles the
/// remaining H1 content as alternating live Lua and prose blocks.
///
/// `content_abs_line` is the 1-based line number in the full input where
/// `content` begins.
///
/// # Errors
/// Returns [`Error::Parse`] for the removed `lua prompt` fence form or an
/// unclosed fence, and a Lua compilation error for invalid Lua.
pub(super) fn split_h1(
    content: &str,
    title: &str,
    content_abs_line: u32,
    execution: &str,
    observer: &dyn Observer,
) -> Result<(Option<LuaProgram>, Vec<Block>, String)> {
    // The H1 takes only the marker's comment role: everything below the first
    // `---` rule is reader-only, so a `lua shared` fence there is inert and
    // the description text comes from above the rule.
    let content = truncate_at_first_rule(content);
    let leading = trim_leading_blank_lines(content);
    if leading.lines().next() == Some("```lua prompt") {
        return Err(Error::Parse(
            "the `lua prompt` fence form was removed; use `lua` for a live H1 block or `lua shared` for the shared library".into(),
        ));
    }

    let shared_opening = exact_shared_openings(content).into_iter().next();
    let mut h1_content = content.as_bytes().to_vec();
    let replay = if let Some(opening) = shared_opening {
        let after_open = strip_exact_shared_opening(&content[opening..]).ok_or_else(|| {
            Error::Parse("internal shared fence classification mismatch".to_owned())
        })?;
        let (source, rest) = extract_exact_fence(after_open, "prompt `lua shared`")?;
        let fence_end = content.len() - rest.len();
        blank_preserving_newlines(&mut h1_content[opening..fence_end]);
        Some(LuaProgram::compile(
            &source,
            "prompt shared library",
            nz_source_line(line_add(
                line_add(content_abs_line, newlines_before(content, opening)?)?,
                1,
            )?)?,
            execution,
            observer,
            title,
        )?)
    } else {
        None
    };

    let h1_content = String::from_utf8(h1_content)
        .map_err(|_| Error::Parse("internal H1 source masking failed".to_owned()))?;
    let raw_blocks = split_section_blocks(&h1_content, title)?;
    let last_prose = raw_blocks
        .iter()
        .rposition(|block| matches!(block, RawBlock::Prose(_)));
    let total = raw_blocks.len();
    let mut blocks = Vec::with_capacity(total);
    for (index, raw) in raw_blocks.into_iter().enumerate() {
        match raw {
            RawBlock::Prose(text) => blocks.push(Block::Prose {
                text,
                loop_capable: Some(index) == last_prose,
            }),
            RawBlock::Lua {
                source,
                line_offset,
            } => {
                let location = format!("H1 `{title}` lua");
                blocks.push(Block::Lua(LuaProgram::compile(
                    &source,
                    &location,
                    nz_source_line(line_add(content_abs_line, line_offset)?)?,
                    execution,
                    observer,
                    title,
                )?));
            }
        }
    }
    if matches!(
        blocks.as_slice(),
        [Block::Prose {
            text,
            loop_capable: _
        }] if text.is_empty()
    ) {
        blocks.clear();
    }
    let description_text = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Prose { text, .. } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok((replay, blocks, description_text))
}

/// Removes complete leading lines that contain only whitespace.
fn trim_leading_blank_lines(content: &str) -> &str {
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
/// line. Near-miss closing markers are part of the source.
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
    Err(Error::Parse(format!("{label} fence is not closed")))
}

/// Removes one exact unindented `lua` opening line from `content`.
fn strip_exact_lua_opening(content: &str) -> Option<&str> {
    content
        .strip_prefix("```lua\r\n")
        .or_else(|| content.strip_prefix("```lua\n"))
        .or_else(|| (content == "```lua").then_some(""))
}

/// Removes one exact unindented `lua shared` opening line from `content`.
fn strip_exact_shared_opening(content: &str) -> Option<&str> {
    content
        .strip_prefix("```lua shared\r\n")
        .or_else(|| content.strip_prefix("```lua shared\n"))
        .or_else(|| (content == "```lua shared").then_some(""))
}

/// Returns byte offsets of top-level fences whose opening line is `marker`.
fn exact_fence_openings(content: &str, marker: &str) -> Vec<usize> {
    Parser::new_ext(content, Options::empty())
        .into_offset_iter()
        .filter_map(|(event, range)| {
            if !matches!(
                event,
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_)))
            ) {
                return None;
            }
            let line_start = content[..range.start]
                .rfind('\n')
                .map_or(0, |newline| newline + 1);
            (content[line_start..].lines().next() == Some(marker)).then_some(line_start)
        })
        .collect()
}

/// Returns the byte offsets of top-level exact `lua` opening lines.
///
/// Using the Markdown event stream keeps marker-looking lines inside longer
/// fences as prose rather than accidentally reserving them.
fn exact_lua_openings(content: &str) -> Vec<usize> {
    exact_fence_openings(content, "```lua")
}

/// Returns the byte offsets of top-level exact `lua shared` opening lines.
pub(super) fn exact_shared_openings(content: &str) -> Vec<usize> {
    exact_fence_openings(content, "```lua shared")
}

/// Byte offset where leading blank lines end.
fn leading_content_start(content: &str) -> usize {
    content.len() - trim_leading_blank_lines(content).len()
}

/// Blanks a byte slice in place, preserving `\r` and `\n` so source-line
/// numbers still map back to the original file.
fn blank_preserving_newlines(bytes: &mut [u8]) {
    for byte in bytes {
        if !matches!(*byte, b'\r' | b'\n') {
            *byte = b' ';
        }
    }
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

/// Truncates a content region at its first `---` rule, making everything
/// below the rule reader-only.
pub(super) fn truncate_at_first_rule(content: &str) -> &str {
    match rule_ranges(content).first() {
        Some(range) => &content[..range.start],
        None => content,
    }
}

/// Applies the `---` marker's two roles to one section's content.
///
/// A rule that precedes any executable content (only whitespace before it)
/// marks the section off-walk: the marker is blanked out and the content
/// below parses normally. Any later rule is then the comment boundary where
/// the section's executable content ends. With no leading rule, the first
/// rule is the comment boundary. The marker is blanked rather than removed so
/// Lua source-line numbers still map back to the original file.
///
/// # Errors
/// Returns [`Error::Parse`] when the masked content fails UTF-8 validation,
/// which the ASCII-only masking makes unreachable.
pub(super) fn split_rule_roles(content: &str) -> Result<(bool, Cow<'_, str>)> {
    let ranges = rule_ranges(content);
    let Some(first) = ranges.first() else {
        return Ok((false, Cow::Borrowed(content)));
    };
    if !content[..first.start].trim().is_empty() {
        return Ok((false, Cow::Borrowed(&content[..first.start])));
    }
    let mut masked = content.as_bytes().to_vec();
    blank_preserving_newlines(&mut masked[first.clone()]);
    let end = ranges.get(1).map_or(content.len(), |range| range.start);
    masked.truncate(end);
    let masked = String::from_utf8(masked)
        .map_err(|_| Error::Parse("internal rule masking failed".to_owned()))?;
    Ok((true, Cow::Owned(masked)))
}

/// Splits a section into alternating exact `lua` fences and prose segments.
///
/// Every exact top-level `lua` fence becomes a Lua block. Text between fences
/// becomes prose (including an empty segment between consecutive fences, so
/// classic prologue/epilog with empty prose stays `[Lua, Prose, Lua]`). Leading
/// blank lines before a leading fence are discarded. Near-miss fence forms stay
/// inside prose.
///
/// # Errors
/// Returns [`Error::Parse`] when an exact `lua` fence is not closed.
pub(super) fn split_section_blocks(content: &str, section: &str) -> Result<Vec<RawBlock>> {
    let openings = exact_lua_openings(content);
    if openings.is_empty() {
        return Ok(vec![RawBlock::Prose(content.trim().to_string())]);
    }

    let leading_start = leading_content_start(content);
    let mut blocks = Vec::new();
    let mut pos = 0usize;

    for (index, &opening) in openings.iter().enumerate() {
        let Some(after_open) = strip_exact_lua_opening(&content[opening..]) else {
            return Err(Error::Parse(
                "internal section fence classification mismatch".to_owned(),
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
            blocks.push(RawBlock::Prose(content[pos..opening].trim().to_string()));
        }

        let line_offset = line_add(newlines_before(content, opening)?, 1)?;
        blocks.push(RawBlock::Lua {
            source,
            line_offset,
        });
        pos = fence_end;
    }

    let trailing = content[pos..].trim();
    if !trailing.is_empty() {
        blocks.push(RawBlock::Prose(trailing.to_string()));
    }

    Ok(blocks)
}

/// Compile label for one Lua block, preserving classic prologue/epilog names.
pub(super) fn lua_block_location(
    section: &str,
    index: usize,
    total: usize,
    has_prose: bool,
) -> String {
    let is_first = index == 0;
    let is_last = index + 1 == total;
    if is_first {
        return format!("section `{section}` prologue");
    }
    if is_last && has_prose {
        return format!("section `{section}` epilog");
    }
    format!("section `{section}` lua")
}
