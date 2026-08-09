//! Prompt file parser.
//!
//! A prompt is one markdown file: YAML frontmatter, a required H1, an optional
//! immediately leading shared `lua` fence, then a sequence of H2 sections.
//! Sections nest recursively (H3 under H2, H4 under H3, and so on through H6).
//! Each section is an alternating sequence of exact `lua` fences and prose
//! ([`Block`]). The last prose block is marked loop-capable at parse time.
//! Classic prologue/prose/epilog is exactly `[Lua, Prose, Lua]`.
//!
//! The parser does no execution. It turns bytes into a [`Prompt`] tree.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::lua::LuaProgram;
use crate::observe::{Observer, detail};
use crate::{Error, Result};

/// The parsed frontmatter of a prompt file.
#[derive(Debug, Clone, serde::Deserialize)]
#[non_exhaustive]
pub struct Frontmatter {
    /// The prompt's identifier, supplied explicitly by a caller.
    pub name: String,
    /// One-line description shown in prompt listings and name retrieval.
    pub description: String,
    /// The promptforge engine major this file targets. Its presence marks the
    /// file as a promptforge prompt; `None` means the file is not one. Optional.
    #[serde(default)]
    pub promptforge: Option<u32>,
    /// Value returned when the run falls off the last section. Optional.
    #[serde(default)]
    pub default_return: Option<String>,
    /// Maximum model round trips a section's tool-call loop may take. Optional;
    /// `None` means the runtime applies its default cap rather than zero.
    #[serde(default)]
    pub max_tool_iterations: Option<usize>,
}

/// One executable block inside a section: a compiled Lua fence or prose.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Block {
    /// An exact `lua` fence compiled at parse time.
    Lua(LuaProgram),
    /// Author prose for the model. `loop_capable` is true only for the last
    /// prose block in the section (full tool loop); earlier prose is single-shot.
    Prose {
        /// Substituted and sent to the model when non-empty.
        text: String,
        /// Whether this prose runs the full tool loop (`true`) or one round.
        loop_capable: bool,
    },
}

/// One section of a prompt: a heading, ordered blocks, and children.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Section {
    /// The heading text (the section's address).
    pub name: String,
    /// The heading level, 2 through 6.
    pub level: u8,
    /// Ordered lua/prose blocks for this section.
    pub blocks: Vec<Block>,
    /// Child sections nested under this one (deeper heading levels).
    pub children: Vec<Section>,
    /// Pre-parsed bullet items for list-only sections (no lua blocks).
    /// Empty for non-list sections.
    pub items: Vec<String>,
}

impl Section {
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

    /// True when this section has no Lua blocks.
    #[must_use]
    pub fn is_list_only(&self) -> bool {
        self.blocks
            .iter()
            .all(|block| matches!(block, Block::Prose { .. }))
    }
}

/// A fully parsed prompt file.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Prompt {
    /// The parsed YAML frontmatter.
    pub frontmatter: Frontmatter,
    /// The required H1 title.
    pub title: String,
    /// The compiled shared prompt library, when the H1 opens with one.
    pub shared: Option<LuaProgram>,
    /// Human-readable description text between the H1 and the first section.
    pub description_text: String,
    /// Top-level sections (H2s) in file order.
    pub sections: Vec<Section>,
}

impl Prompt {
    /// Parse a prompt file's full source text into a [`Prompt`].
    ///
    /// Every parse and compilation report carries the caller-provided
    /// `execution` identifier unchanged.
    ///
    /// # Errors
    /// Returns [`Error::Parse`] when the frontmatter delimiters are missing, the
    /// frontmatter is invalid, the required H1 is missing, the H1 opens with the
    /// removed `lua prompt` fence form, a reserved fence is not closed exactly,
    /// or the body has no `##` sections. Returns [`Error::LuaCompile`] when the
    /// shared library or a section Lua block is not valid Lua.
    pub fn parse(input: &str, execution: &str, observer: &dyn Observer) -> Result<Prompt> {
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
        result
    }

    fn parse_inner(input: &str, execution: &str, observer: &dyn Observer) -> Result<Prompt> {
        let (yaml, body, frontmatter_lines) = split_frontmatter(input)?;
        let frontmatter: Frontmatter = serde_yaml::from_str(&yaml)
            .map_err(|e| Error::Parse(format!("invalid frontmatter: {e}")))?;

        let headings = collect_headings(&body);

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
        let h1_content_abs_line = frontmatter_lines + h1.content_start_line;
        let (shared, description_text) = split_shared(
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
            shared,
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
fn newlines_before(text: &str, byte_offset: usize) -> u32 {
    u32::try_from(text[..byte_offset].matches('\n').count()).unwrap_or(u32::MAX)
}

/// Counts the number of lines in `text` (number of `\n` characters).
fn count_lines(text: &str) -> u32 {
    u32::try_from(text.matches('\n').count()).unwrap_or(u32::MAX)
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
fn collect_headings(body: &str) -> Vec<Heading> {
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
        let content_start_line = newlines_before(body, start) + 1;
        headings.push(Heading {
            level: raws[i].level,
            title: raws[i].title.clone(),
            content: content.to_string(),
            content_start_line,
        });
    }
    headings
}

/// Extracts and compiles the optional exact shared library at the start of the
/// H1 content. Only blank lines may precede the opening fence; a later exact
/// `lua` fence is ordinary description prose, mirroring section semantics.
///
/// `content_abs_line` is the 1-based line number in the full input where
/// `content` begins.
fn split_shared(
    content: &str,
    title: &str,
    content_abs_line: u32,
    execution: &str,
    observer: &dyn Observer,
) -> Result<(Option<LuaProgram>, String)> {
    let leading = trim_leading_blank_lines(content);
    if leading.lines().next() == Some("```lua prompt") {
        return Err(Error::Parse(
            "the `lua prompt` fence form was removed; open the shared library with a plain `lua` fence".into(),
        ));
    }
    let Some(after_open) = strip_exact_lua_opening(leading) else {
        return Ok((None, content.trim().to_string()));
    };

    // The Lua source starts on the line after the ```lua opener.
    // Blank lines skipped + the ```lua line itself.
    let blank_lines = count_lines(content) - count_lines(leading);
    let lua_source_line = content_abs_line + blank_lines + 1; // +1 for the ```lua line

    let (source, rest) = extract_exact_fence(after_open, "shared `lua`")?;
    let program = LuaProgram::compile(
        &source,
        "prompt shared library",
        lua_source_line,
        execution,
        observer,
        title,
    )?;
    Ok((Some(program), rest.trim().to_string()))
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

/// Returns the byte offsets of top-level exact `lua` opening lines.
///
/// Using the Markdown event stream keeps marker-looking lines inside longer
/// fences as prose rather than accidentally reserving them.
fn exact_lua_openings(content: &str) -> Vec<usize> {
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
            (content[line_start..].lines().next() == Some("```lua")).then_some(line_start)
        })
        .collect()
}

/// Uncompiled block produced while scanning section content.
enum RawBlock {
    Lua {
        source: String,
        /// Lines before the first line of Lua source within section content.
        line_offset: u32,
    },
    Prose(String),
}

/// Byte offset where leading blank lines end.
fn leading_content_start(content: &str) -> usize {
    content.len() - trim_leading_blank_lines(content).len()
}

/// Splits a section into alternating exact `lua` fences and prose segments.
///
/// Every exact top-level `lua` fence becomes a Lua block. Text between fences
/// becomes prose (including an empty segment between consecutive fences, so
/// classic prologue/epilog with empty prose stays `[Lua, Prose, Lua]`). Leading
/// blank lines before a leading fence are discarded. Near-miss fence forms stay
/// inside prose.
fn split_section_blocks(content: &str, section: &str) -> Result<Vec<RawBlock>> {
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

        let line_offset = newlines_before(content, opening) + 1;
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
fn lua_block_location(section: &str, index: usize, total: usize, has_prose: bool) -> String {
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

/// Parses bullet items from prose in a list-only section.
///
/// A list-only section has no Lua blocks. Its prose must contain only unordered
/// (`- ` or `* `) or ordered (`N. ` or `N) `) bullet lines, with blank lines
/// ignored. Returns an error if the section contains non-list content or if the
/// items list is empty.
pub(crate) fn parse_bullet_items(prose: &str, section: &str) -> Result<Vec<String>> {
    let mut items = Vec::new();
    for line in prose.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            if rest.trim().is_empty() {
                return Err(Error::Parse(format!(
                    "empty bullet item in list section `{section}`"
                )));
            }
            items.push(rest.to_string());
        } else if trimmed == "-" || trimmed == "*" {
            return Err(Error::Parse(format!(
                "empty bullet item in list section `{section}`"
            )));
        } else if let Some(item) = strip_ordered_marker(trimmed) {
            if item.trim().is_empty() {
                return Err(Error::Parse(format!(
                    "empty bullet item in list section `{section}`"
                )));
            }
            items.push(item.to_string());
        } else {
            return Err(Error::Parse(format!(
                "section `{section}` is a list section but contains non-list content: {trimmed}"
            )));
        }
    }
    if items.is_empty() {
        return Err(Error::Parse(format!(
            "section `{section}` is a list section but has no items"
        )));
    }
    Ok(items)
}

/// Strips an ordered-list marker (`N. ` or `N) `) and returns the remainder.
fn strip_ordered_marker(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let rest = &line[i..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

/// Build a section tree from a flat, document-ordered list of headings.
///
/// Recursion consumes headings whose level is deeper than `parent_level`; a
/// heading at or above `parent_level` belongs to an ancestor and stops the
/// current level. Skipped heading levels are tolerated: an H4 following an H2
/// simply becomes a child of the H2.
fn build_sections(
    headings: &[Heading],
    pos: &mut usize,
    parent_level: u8,
    frontmatter_lines: u32,
    execution: &str,
    observer: &dyn Observer,
) -> Result<Vec<Section>> {
    let mut result = Vec::new();
    while *pos < headings.len() {
        let level = headings[*pos].level;
        if level <= parent_level {
            break;
        }
        let h = &headings[*pos];
        let name = h.title.clone();
        let content_abs_line = frontmatter_lines + h.content_start_line;
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
                    let abs_line = content_abs_line + line_offset;
                    let location = lua_block_location(&name, index, total, has_prose);
                    let program = LuaProgram::compile(
                        &source, &location, abs_line, execution, observer, &name,
                    )?;
                    blocks.push(Block::Lua(program));
                }
            }
        }
        *pos += 1;
        let children =
            build_sections(headings, pos, level, frontmatter_lines, execution, observer)?;

        let is_list_only = blocks
            .iter()
            .all(|block| matches!(block, Block::Prose { .. }));
        let prose_for_items = blocks.iter().find_map(|block| match block {
            Block::Prose { text, .. } => Some(text.as_str()),
            Block::Lua(_) => None,
        });
        let items = if is_list_only
            && let Some(prose) = prose_for_items
            && has_any_bullet_line(prose)
        {
            parse_bullet_items(prose, &name)?
        } else {
            Vec::new()
        };

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

/// Returns true if the prose contains at least one bullet-list line.
fn has_any_bullet_line(prose: &str) -> bool {
    prose.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || strip_ordered_marker(trimmed).is_some()
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::observe::{NullObserver, detail};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, execution: &str, section: &str, detail: &str) {
            self.0
                .lock()
                .expect("recording lock must remain usable")
                .push((
                    execution.to_string(),
                    section.to_string(),
                    detail.to_string(),
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
        assert!(p.shared.is_none());
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
        let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n \t\r\n```lua\r\nfunction answer() return 42 end\r\n```\r\n\r\nDescription.\r\n\r\n## S\r\n\r\np\r\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("immediate shared Lua must parse");
        let shared = prompt.shared.expect("shared program must be present");
        assert_eq!(shared.source(), "function answer() return 42 end");
        assert_eq!(prompt.description_text, "Description.");
    }

    #[test]
    fn h1_lua_fence_after_prose_or_shared_library_remains_prose() {
        let after_prose = "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro prose.\n\n```lua\nnot compiled =\n```\n\n## S\n\np\n";
        let prompt = Prompt::parse(after_prose, "test", &NullObserver)
            .expect("a post-prose H1 Lua fence is model-facing prose");
        assert!(prompt.shared.is_none());
        assert_eq!(
            prompt.description_text,
            "Intro prose.\n\n```lua\nnot compiled =\n```"
        );

        let after_shared = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal a = 1\n```\n\n```lua\nnot compiled =\n```\n\n## S\n\np\n";
        let prompt = Prompt::parse(after_shared, "test", &NullObserver)
            .expect("only the leading H1 Lua fence is reserved");
        let shared = prompt
            .shared
            .expect("the leading fence is the shared library");
        assert_eq!(shared.source(), "local a = 1");
        assert_eq!(prompt.description_text, "```lua\nnot compiled =\n```");
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
        assert!(prompt.shared.is_none());
        assert!(prompt.description_text.contains("```lua prompt"));

        let in_section = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua prompt\nnot compiled =\n```\n";
        let prompt = Prompt::parse(in_section, "test", &NullObserver)
            .expect("the removed form in a section is ordinary Markdown");
        assert!(prompt.entry().prologue().is_none());
        assert!(prompt.entry().prose().contains("```lua prompt"));
    }

    #[test]
    fn shared_fence_markers_must_be_exact() {
        // Leading position is the discriminating placement: only the exact
        // ```lua opener is reserved there, so each near-miss must stay prose.
        // The removed ```lua prompt form is excluded because leading it is a
        // targeted error, pinned by
        // `removed_lua_prompt_form_is_a_targeted_error_when_leading`.
        for near_miss in [
            "````lua\nreturn 1\n````",
            " ```lua\nreturn 1\n ```",
            "```Lua\nreturn 1\n```",
            "```lua extra\nreturn 1\n```",
        ] {
            let src =
                format!("---\nname: x\ndescription: d\n---\n\n# T\n\n{near_miss}\n\n## S\n\np\n");
            let prompt = Prompt::parse(&src, "test", &NullObserver)
                .expect("leading near-miss shared markers must remain prose");
            assert!(prompt.shared.is_none());
            assert!(prompt.description_text.contains(near_miss.trim()));
        }

        // Post-prose position: everything is prose there, near-miss or not.
        for near_miss in [
            "````lua\nreturn 1\n````",
            " ```lua\nreturn 1\n ```",
            "```Lua\nreturn 1\n```",
            "```lua extra\nreturn 1\n```",
            "```lua prompt\nreturn 1\n```",
        ] {
            let src = format!(
                "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro.\n\n{near_miss}\n\n## S\n\np\n"
            );
            let prompt = Prompt::parse(&src, "test", &NullObserver)
                .expect("near-miss shared markers must remain prose");
            assert!(prompt.shared.is_none());
            assert!(prompt.description_text.contains(near_miss));
        }

        let unclosed =
            "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nreturn 1\n````\n\n## S\n\np\n";
        let error = Prompt::parse(unclosed, "test", &NullObserver)
            .expect_err("near-miss closing marker must not close the fence");
        assert!(error.to_string().contains("not closed"));
    }

    #[test]
    fn shared_markers_inside_longer_fences_remain_prose() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n````markdown\n```lua\nreturn 1\n```\n````\n\nIntro.\n\n## S\n\n````markdown\n```lua\nreturn 2\n```\n````\n";
        let prompt = Prompt::parse(src, "test", &NullObserver)
            .expect("nested shared markers must remain prose");

        assert!(prompt.shared.is_none());
        assert!(prompt.description_text.contains("```lua"));
        assert!(prompt.sections[0].prologue().is_none());
        assert!(prompt.sections[0].prose().contains("```lua"));
    }

    #[test]
    fn malformed_shared_lua_retains_diagnostics_and_reports_safe_boundaries() {
        let recorder = Recorder::default();
        let source = "private_payload =";
        let src = format!(
            "---\nname: x\ndescription: d\n---\n\n# Private title\n\n```lua\n{source}\n```\n\n## S\n\np\n"
        );
        let error = Prompt::parse(&src, "parse-failure", &recorder)
            .expect_err("malformed shared Lua must fail");
        match error {
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
                ("Prompt".into(), detail::PARSE_STARTED.into()),
                (
                    "Private title".into(),
                    detail::LUA_COMPILATION_STARTED.into()
                ),
                (
                    "Private title".into(),
                    detail::LUA_COMPILATION_FAILED.into()
                ),
                ("Prompt".into(), detail::PARSE_FAILED.into()),
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
                ("Prompt".into(), detail::PARSE_STARTED.into()),
                ("T".into(), detail::LUA_COMPILATION_STARTED.into()),
                ("T".into(), detail::LUA_COMPILATION_SUCCEEDED.into()),
                ("Prompt".into(), detail::PARSE_SUCCEEDED.into()),
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
            match error {
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
                    .map(|(_, detail)| detail.as_str())
                    .collect::<Vec<_>>(),
                expected_details
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
                ("Prompt".into(), detail::PARSE_STARTED.into()),
                ("S".into(), detail::LUA_COMPILATION_STARTED.into()),
                ("S".into(), detail::LUA_COMPILATION_SUCCEEDED.into()),
                ("S".into(), detail::LUA_COMPILATION_STARTED.into()),
                ("S".into(), detail::LUA_COMPILATION_SUCCEEDED.into()),
                ("Prompt".into(), detail::PARSE_SUCCEEDED.into()),
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
    fn skipped_heading_level_tolerated() {
        // H4 directly under H2 (no H3) becomes a direct child of the H2.
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n#### D\n\nd\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        let a = &p.sections[0];
        assert_eq!(a.children.len(), 1);
        assert_eq!(a.children[0].name, "D");
        assert_eq!(a.children[0].level, 4);
    }

    #[test]
    fn max_tool_iterations_parses_when_declared_and_defaults_to_none() {
        let declared =
            "---\nname: x\ndescription: d\nmax_tool_iterations: 20\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(declared, "test", &NullObserver).unwrap();
        assert_eq!(p.frontmatter.max_tool_iterations, Some(20));

        let absent = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(absent, "test", &NullObserver).unwrap();
        assert_eq!(p.frontmatter.max_tool_iterations, None);
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

        assert_eq!(epilog.source_line(), 13, "epilog Lua starts on line 13");
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

        assert_eq!(prologue.source_line(), 11, "prologue Lua starts on line 11");

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

        assert_eq!(epilog.source_line(), 13);

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
        //  8: ```lua        <- shared opens
        //  9: function f()  <- source_line = 9
        // 10: end
        // 11: ```
        // 12: (empty)
        // 13: ## S
        // 14: (empty)
        // 15: p
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nfunction f()\nend\n```\n\n## S\n\np\n";
        let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
        let shared = prompt.shared.as_ref().expect("shared must exist");
        assert_eq!(shared.source_line(), 9, "shared Lua starts on line 9");
    }
}
