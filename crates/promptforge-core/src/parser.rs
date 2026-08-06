//! Prompt file parser.
//!
//! A prompt is one markdown file: YAML frontmatter, a required H1, an optional
//! immediately leading shared `lua prompt` fence, then a sequence of H2
//! sections.
//! Sections nest recursively (H3 under H2, H4 under H3, and so on through H6),
//! and each section may begin with a single leading `lua` code fence that the
//! parser separates from the prose.
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
    /// The prompt's identifier (becomes the MCP tool name).
    pub name: String,
    /// One-line description; steers a calling model's tool selection.
    pub description: String,
    /// Contract version, bumped when the interface changes.
    pub version: u32,
    /// The promptforge engine major this file targets, distinct from the
    /// author-facing [`version`](Self::version). Its presence marks the file as
    /// a promptforge prompt; `None` means the file is not one. Optional.
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

/// One section of a prompt: a heading, an optional Lua block, prose, and any
/// nested child sections.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Section {
    /// The heading text (the section's address).
    pub name: String,
    /// The heading level, 2 through 6.
    pub level: u8,
    /// The leading `lua` code fence, if the section started with one.
    pub lua: Option<String>,
    /// The prose the model reads, with any leading Lua fence removed.
    pub prose: String,
    /// Child sections nested under this one (deeper heading levels).
    pub children: Vec<Section>,
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
    /// # Errors
    /// Returns [`Error::Parse`] when the frontmatter delimiters are missing, the
    /// frontmatter is invalid, the required H1 is missing, a shared library is
    /// misplaced, or the body has no `##` sections. Returns
    /// [`Error::LuaCompile`] when the shared library is not valid Lua.
    pub fn parse(input: &str, observer: &dyn Observer) -> Result<Prompt> {
        observer.observe("Prompt", detail::PARSE_STARTED);
        let result = Self::parse_inner(input, observer);
        observer.observe(
            "Prompt",
            if result.is_ok() {
                detail::PARSE_SUCCEEDED
            } else {
                detail::PARSE_FAILED
            },
        );
        result
    }

    fn parse_inner(input: &str, observer: &dyn Observer) -> Result<Prompt> {
        let (yaml, body) = split_frontmatter(input)?;
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
        let (shared, description_text) = split_shared(&h1.content, &title, observer)?;

        // Everything before the H1 is preface and has no prompt semantics.
        // Sections are headings after the H1 at level 2 or deeper.
        let section_headings: Vec<Heading> = headings
            .into_iter()
            .skip(*h1_index + 1)
            .filter(|h| h.level >= 2)
            .collect();
        if section_headings
            .iter()
            .any(|heading| has_exact_shared_opening(&heading.content))
        {
            return Err(Error::Parse(if shared.is_some() {
                "prompt contains more than one shared `lua prompt` fence".into()
            } else {
                "shared `lua prompt` fence must immediately follow the H1".into()
            }));
        }
        if section_headings.is_empty() {
            return Err(Error::Parse("prompt has no ## sections".into()));
        }
        let mut pos = 0;
        let sections = build_sections(&section_headings, &mut pos, 1);

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
}

/// Split a file into its YAML frontmatter and its markdown body.
///
/// The file must open with a `---` line and close the frontmatter with another
/// `---` line. `str::lines` handles both `\n` and `\r\n`.
fn split_frontmatter(input: &str) -> Result<(String, String)> {
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
    for line in lines.by_ref() {
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
    Ok((yaml, body))
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

    let (yaml, _body) = split_frontmatter(source).ok()?;
    let probe: Probe = serde_yaml::from_str(&yaml).ok()?;
    probe.promptforge
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
        headings.push(Heading {
            level: raws[i].level,
            title: raws[i].title.clone(),
            content: content.to_string(),
        });
    }
    headings
}

/// Extracts and compiles the optional exact shared library at the start of the
/// H1 content. Only blank lines may precede the opening fence.
fn split_shared(
    content: &str,
    title: &str,
    observer: &dyn Observer,
) -> Result<(Option<LuaProgram>, String)> {
    let leading = trim_leading_blank_lines(content);
    let after_open = leading
        .strip_prefix("```lua prompt\r\n")
        .or_else(|| leading.strip_prefix("```lua prompt\n"))
        .or_else(|| (leading == "```lua prompt").then_some(""));
    let Some(after_open) = after_open else {
        if has_exact_shared_opening(content) {
            return Err(Error::Parse(
                "shared `lua prompt` fence must immediately follow the H1".into(),
            ));
        }
        return Ok((None, content.trim().to_string()));
    };

    let (source, rest) = extract_exact_fence(after_open, "shared `lua prompt`")?;
    if has_exact_shared_opening(rest) {
        return Err(Error::Parse(
            "prompt contains more than one shared `lua prompt` fence".into(),
        ));
    }
    let program = LuaProgram::compile(&source, "prompt shared library", observer, title)?;
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

/// Whether `content` contains an exact reserved shared-library opening line.
fn has_exact_shared_opening(content: &str) -> bool {
    Parser::new_ext(content, Options::empty())
        .into_offset_iter()
        .any(|(event, range)| {
            let line_start = content[..range.start]
                .rfind('\n')
                .map_or(0, |newline| newline + 1);
            matches!(
                event,
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_)))
            ) && content[line_start..].lines().next() == Some("```lua prompt")
        })
}

/// If `content` begins (after leading whitespace) with a `lua` code fence,
/// extract the Lua source and return it separately from the remaining prose. A
/// code fence in any other language, or one not at the start, stays in the prose.
fn split_lua(content: &str) -> (Option<String>, String) {
    let mut lines = content.trim_start().lines();
    let first = lines.next().unwrap_or("");
    let lang = first.trim().strip_prefix("```").map(str::trim);
    if lang.is_none_or(|l| !l.eq_ignore_ascii_case("lua")) {
        return (None, content.trim().to_string());
    }

    let mut lua = String::new();
    let mut rest = String::new();
    let mut closed = false;
    for line in lines {
        if !closed && line.trim() == "```" {
            closed = true;
            continue;
        }
        if closed {
            rest.push_str(line);
            rest.push('\n');
        } else {
            lua.push_str(line);
            lua.push('\n');
        }
    }

    if !closed {
        // Unterminated fence: treat the whole thing as prose rather than guess.
        return (None, content.trim().to_string());
    }
    (Some(lua.trim_end().to_string()), rest.trim().to_string())
}

/// Build a section tree from a flat, document-ordered list of headings.
///
/// Recursion consumes headings whose level is deeper than `parent_level`; a
/// heading at or above `parent_level` belongs to an ancestor and stops the
/// current level. Skipped heading levels are tolerated: an H4 following an H2
/// simply becomes a child of the H2.
fn build_sections(headings: &[Heading], pos: &mut usize, parent_level: u8) -> Vec<Section> {
    let mut result = Vec::new();
    while *pos < headings.len() {
        let level = headings[*pos].level;
        if level <= parent_level {
            break;
        }
        let h = &headings[*pos];
        let name = h.title.clone();
        let (lua, prose) = split_lua(&h.content);
        *pos += 1;
        let children = build_sections(headings, pos, level);
        result.push(Section {
            name,
            level,
            lua,
            prose,
            children,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::observe::{NullObserver, detail};

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, String)>>);

    impl Observer for Recorder {
        fn observe(&self, section: &str, detail: &str) {
            self.0
                .lock()
                .expect("recording lock must remain usable")
                .push((section.to_string(), detail.to_string()));
        }
    }

    impl Recorder {
        fn observations(&self) -> Vec<(String, String)> {
            self.0
                .lock()
                .expect("recording lock must remain usable")
                .clone()
        }
    }

    #[test]
    fn parses_multi_section_with_all_features() {
        let src = "---\n\
name: demo\n\
description: A demo\n\
version: 2\n\
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

        let p = Prompt::parse(src, &NullObserver).unwrap();
        assert_eq!(p.frontmatter.name, "demo");
        assert_eq!(p.frontmatter.description, "A demo");
        assert_eq!(p.frontmatter.version, 2);
        assert_eq!(p.title, "Demo Title");
        assert!(p.shared.is_none());
        assert_eq!(p.description_text, "Human-readable intro text.");

        assert_eq!(p.sections.len(), 2);
        let first = &p.sections[0];
        assert_eq!(first.name, "First");
        assert_eq!(first.level, 2);
        assert_eq!(first.lua.as_deref(), Some("local x = 1"));
        assert_eq!(first.prose, "Prose for the first section.");
        assert_eq!(first.children.len(), 1);
        assert_eq!(first.children[0].name, "Child");
        assert_eq!(first.children[0].level, 3);
        assert_eq!(first.children[0].prose, "Child prose.");

        assert_eq!(p.sections[1].name, "Second");
        assert!(p.sections[1].lua.is_none());
    }

    #[test]
    fn parses_single_minimal_section() {
        let src = "---\nname: hi\ndescription: d\nversion: 1\n---\n\n# T\n\n## Greet\n\nSay hi\n";
        let p = Prompt::parse(src, &NullObserver).unwrap();
        assert_eq!(p.sections.len(), 1);
        assert_eq!(p.sections[0].name, "Greet");
        assert_eq!(p.sections[0].prose, "Say hi");
        assert!(p.frontmatter.default_return.is_none());
    }

    #[test]
    fn missing_required_frontmatter_field_errors() {
        // No `version`.
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
        assert!(Prompt::parse(src, &NullObserver).is_err());
    }

    #[test]
    fn missing_frontmatter_delimiter_errors() {
        let src = "# T\n\n## S\n\np\n";
        assert!(Prompt::parse(src, &NullObserver).is_err());
    }

    #[test]
    fn no_sections_errors() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# Only a title\n\nText.\n";
        assert!(Prompt::parse(src, &NullObserver).is_err());
    }

    #[test]
    fn missing_h1_errors() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\nPreface.\n\n## S\n\np\n";
        let error = Prompt::parse(src, &NullObserver).expect_err("H1 must be required");
        assert!(error.to_string().contains("requires an H1"));
    }

    #[test]
    fn empty_h1_title_errors() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n#\n\n## S\n\np\n";
        let error = Prompt::parse(src, &NullObserver).expect_err("H1 title must not be empty");
        assert!(error.to_string().contains("title must not be empty"));
    }

    #[test]
    fn preface_before_h1_is_ignored() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\nIgnored preface.\n\n```text\nalso ignored\n```\n\n# T\n\nDescription.\n\n## S\n\np\n";
        let prompt = Prompt::parse(src, &NullObserver).expect("preface is not semantic");
        assert_eq!(prompt.title, "T");
        assert_eq!(prompt.description_text, "Description.");
        assert_eq!(prompt.entry().name, "S");
    }

    #[test]
    fn shared_library_allows_blank_lines_and_is_compiled() {
        let src = "---\r\nname: x\r\ndescription: d\r\nversion: 1\r\n---\r\n\r\n# T\r\n\r\n \t\r\n```lua prompt\r\nfunction answer() return 42 end\r\n```\r\n\r\nDescription.\r\n\r\n## S\r\n\r\np\r\n";
        let prompt = Prompt::parse(src, &NullObserver).expect("immediate shared Lua must parse");
        let shared = prompt.shared.expect("shared program must be present");
        assert_eq!(shared.source(), "function answer() return 42 end");
        assert_eq!(prompt.description_text, "Description.");
    }

    #[test]
    fn shared_library_after_description_is_misplaced() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\nDescription first.\n\n```lua prompt\nreturn 1\n```\n\n## S\n\np\n";
        let error = Prompt::parse(src, &NullObserver).expect_err("shared Lua must lead H1 content");
        assert!(error.to_string().contains("immediately follow the H1"));
    }

    #[test]
    fn duplicate_shared_libraries_error() {
        let in_h1 = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n```lua prompt\nlocal a = 1\n```\n\n```lua prompt\nlocal b = 2\n```\n\n## S\n\np\n";
        let error =
            Prompt::parse(in_h1, &NullObserver).expect_err("a second H1 library must be rejected");
        assert!(error.to_string().contains("more than one"));

        let in_section = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n```lua prompt\nlocal a = 1\n```\n\n## S\n\n```lua prompt\nlocal b = 2\n```\n";
        let error = Prompt::parse(in_section, &NullObserver)
            .expect_err("a section library must be rejected");
        assert!(error.to_string().contains("more than one"));
    }

    #[test]
    fn shared_fence_markers_must_be_exact() {
        for near_miss in [
            "````lua prompt\nreturn 1\n````",
            " ```lua prompt\nreturn 1\n ```",
            "```Lua prompt\nreturn 1\n```",
            "```lua prompt extra\nreturn 1\n```",
        ] {
            let src = format!(
                "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\nIntro.\n\n{near_miss}\n\n## S\n\np\n"
            );
            let prompt = Prompt::parse(&src, &NullObserver)
                .expect("near-miss shared markers must remain prose");
            assert!(prompt.shared.is_none());
            assert!(prompt.description_text.contains(near_miss));
        }

        let unclosed = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n```lua prompt\nreturn 1\n````\n\n## S\n\np\n";
        let error = Prompt::parse(unclosed, &NullObserver)
            .expect_err("near-miss closing marker must not close the fence");
        assert!(error.to_string().contains("not closed"));
    }

    #[test]
    fn shared_markers_inside_longer_fences_remain_prose() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n````markdown\n```lua prompt\nreturn 1\n```\n````\n\nIntro.\n\n## S\n\n````markdown\n```lua prompt\nreturn 2\n```\n````\n";
        let prompt =
            Prompt::parse(src, &NullObserver).expect("nested shared markers must remain prose");

        assert!(prompt.shared.is_none());
        assert!(prompt.description_text.contains("```lua prompt"));
        assert!(prompt.sections[0].lua.is_none());
        assert!(prompt.sections[0].prose.contains("```lua prompt"));
    }

    #[test]
    fn malformed_shared_lua_retains_diagnostics_and_reports_safe_boundaries() {
        let recorder = Recorder::default();
        let source = "private_payload =";
        let src = format!(
            "---\nname: x\ndescription: d\nversion: 1\n---\n\n# Private title\n\n```lua prompt\n{source}\n```\n\n## S\n\np\n"
        );
        let error = Prompt::parse(&src, &recorder).expect_err("malformed shared Lua must fail");
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
        let source = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n```lua prompt\nlocal secret = 42\n```\n\n## S\n\np\n";
        Prompt::parse(source, &recorder).expect("prompt must parse");
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
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n## S\n\n```lua\nreturn 42\n```\n\nActual prose here.\n";
        let p = Prompt::parse(src, &NullObserver).unwrap();
        assert_eq!(p.sections[0].lua.as_deref(), Some("return 42"));
        assert_eq!(p.sections[0].prose, "Actual prose here.");
    }

    #[test]
    fn non_lua_fence_stays_in_prose() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n## S\n\nHere is code:\n\n```python\nprint(1)\n```\n";
        let p = Prompt::parse(src, &NullObserver).unwrap();
        assert!(p.sections[0].lua.is_none());
        assert!(p.sections[0].prose.contains("```python"));
    }

    #[test]
    fn recursive_nesting_h2_h3_h4() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n## A\n\na\n\n### B\n\nb\n\n#### C\n\nc\n";
        let p = Prompt::parse(src, &NullObserver).unwrap();
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
        let src =
            "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n## A\n\na\n\n#### D\n\nd\n";
        let p = Prompt::parse(src, &NullObserver).unwrap();
        let a = &p.sections[0];
        assert_eq!(a.children.len(), 1);
        assert_eq!(a.children[0].name, "D");
        assert_eq!(a.children[0].level, 4);
    }

    #[test]
    fn max_tool_iterations_parses_when_declared_and_defaults_to_none() {
        let declared = "---\nname: x\ndescription: d\nversion: 1\nmax_tool_iterations: 20\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(declared, &NullObserver).unwrap();
        assert_eq!(p.frontmatter.max_tool_iterations, Some(20));

        let absent = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(absent, &NullObserver).unwrap();
        assert_eq!(p.frontmatter.max_tool_iterations, None);
    }

    #[test]
    fn first_h2_is_entry_regardless_of_name() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n## Zebra\n\nfirst\n\n## Main\n\nsecond\n";
        let p = Prompt::parse(src, &NullObserver).unwrap();
        assert_eq!(p.entry().name, "Zebra");
    }

    #[test]
    fn detection_reads_promptforge_major() {
        let src = "---\nname: x\ndescription: d\nversion: 1\npromptforge: 1\n---\n\n## S\n\np\n";
        assert_eq!(promptforge_version(src), Some(1));
    }

    #[test]
    fn detection_needs_only_the_promptforge_key() {
        // No name/description/version, but the key is present.
        let src = "---\npromptforge: 2\n---\n\n## S\n\np\n";
        assert_eq!(promptforge_version(src), Some(2));
    }

    #[test]
    fn detection_absent_key_is_none() {
        let src = "---\nname: x\ndescription: d\nversion: 1\n---\n\n## S\n\np\n";
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
        let with =
            "---\nname: x\ndescription: d\nversion: 1\npromptforge: 1\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(with, &NullObserver).unwrap();
        assert_eq!(p.frontmatter.promptforge, Some(1));

        let without = "---\nname: x\ndescription: d\nversion: 1\n---\n\n# T\n\n## S\n\np\n";
        let p = Prompt::parse(without, &NullObserver).unwrap();
        assert_eq!(p.frontmatter.promptforge, None);
    }
}
