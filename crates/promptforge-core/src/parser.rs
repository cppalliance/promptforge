//! Prompt file parser.
//!
//! A prompt is one markdown file: YAML frontmatter, a required H1, an optional
//! immediately leading shared `lua prompt` fence, then a sequence of H2
//! sections.
//! Sections nest recursively (H3 under H2, H4 under H3, and so on through H6).
//! Each section has an optional exact leading `lua` preamble fence, prose, and
//! an optional exact trailing `lua` epilog fence.
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

/// One section of a prompt: a heading, compiled Lua around prose, and children.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Section {
    /// The heading text (the section's address).
    pub name: String,
    /// The heading level, 2 through 6.
    pub level: u8,
    /// The compiled exact leading `lua` fence, when present.
    pub preamble: Option<LuaProgram>,
    /// The prose the model reads, between any reserved leading and trailing fences.
    pub prose: String,
    /// The compiled exact trailing `lua` fence, when present.
    pub epilog: Option<LuaProgram>,
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
    /// Every parse and compilation report carries the caller-provided
    /// `execution` identifier unchanged.
    ///
    /// # Errors
    /// Returns [`Error::Parse`] when the frontmatter delimiters are missing, the
    /// frontmatter is invalid, the required H1 is missing, a shared library is
    /// misplaced, a reserved fence is not closed exactly, or the body has no
    /// `##` sections. Returns [`Error::LuaCompile`] when the shared library or a
    /// section preamble or epilog is not valid Lua.
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
        let (shared, description_text) = split_shared(&h1.content, &title, execution, observer)?;

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
        let sections = build_sections(&section_headings, &mut pos, 1, execution, observer)?;

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
    execution: &str,
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
    let program =
        LuaProgram::compile(&source, "prompt shared library", execution, observer, title)?;
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

/// Splits a section into exact leading and trailing Lua source around prose.
///
/// A lone reserved fence is the preamble for compatibility. Exact Lua fences
/// between prose remain prose, as do all near-miss fence forms.
fn split_section_phases(
    content: &str,
    section: &str,
) -> Result<(Option<String>, String, Option<String>)> {
    let leading = trim_leading_blank_lines(content);
    let (preamble, remainder) = if let Some(after_open) = strip_exact_lua_opening(leading) {
        let label = format!("section `{section}` preamble `lua`");
        let (source, rest) = extract_exact_fence(after_open, &label)?;
        (Some(source), rest)
    } else {
        (None, content)
    };

    let mut epilog = None;
    let mut prose = remainder;
    if let Some(opening) = exact_lua_openings(remainder).last().copied() {
        let Some(after_open) = strip_exact_lua_opening(&remainder[opening..]) else {
            return Err(Error::Parse(
                "internal section fence classification mismatch".to_owned(),
            ));
        };
        let label = format!("section `{section}` epilog `lua`");
        match extract_exact_fence(after_open, &label) {
            Ok((source, rest)) if rest.trim().is_empty() => {
                epilog = Some(source);
                prose = &remainder[..opening];
            }
            Ok(_) => {}
            Err(error) => return Err(error),
        }
    }

    Ok((preamble, prose.trim().to_string(), epilog))
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
        let (preamble_source, prose, epilog_source) = split_section_phases(&h.content, &name)?;
        let preamble = preamble_source
            .map(|source| {
                LuaProgram::compile(
                    &source,
                    &format!("section `{name}` preamble"),
                    execution,
                    observer,
                    &name,
                )
            })
            .transpose()?;
        let epilog = epilog_source
            .map(|source| {
                LuaProgram::compile(
                    &source,
                    &format!("section `{name}` epilog"),
                    execution,
                    observer,
                    &name,
                )
            })
            .transpose()?;
        *pos += 1;
        let children = build_sections(headings, pos, level, execution, observer)?;
        result.push(Section {
            name,
            level,
            preamble,
            prose,
            epilog,
            children,
        });
    }
    Ok(result)
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
            first.preamble.as_ref().map(LuaProgram::source),
            Some("local x = 1")
        );
        assert_eq!(first.prose, "Prose for the first section.");
        assert!(first.epilog.is_none());
        assert_eq!(first.children.len(), 1);
        assert_eq!(first.children[0].name, "Child");
        assert_eq!(first.children[0].level, 3);
        assert_eq!(first.children[0].prose, "Child prose.");

        assert_eq!(p.sections[1].name, "Second");
        assert!(p.sections[1].preamble.is_none());
        assert!(p.sections[1].epilog.is_none());
    }

    #[test]
    fn parses_single_minimal_section() {
        let src = "---\nname: hi\ndescription: d\n---\n\n# T\n\n## Greet\n\nSay hi\n";
        let p = Prompt::parse(src, "test", &NullObserver).unwrap();
        assert_eq!(p.sections.len(), 1);
        assert_eq!(p.sections[0].name, "Greet");
        assert_eq!(p.sections[0].prose, "Say hi");
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
    fn missing_h1_errors() {
        let src = "---\nname: x\ndescription: d\n---\n\nPreface.\n\n## S\n\np\n";
        let error = Prompt::parse(src, "test", &NullObserver).expect_err("H1 must be required");
        assert!(error.to_string().contains("requires an H1"));
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
        let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n \t\r\n```lua prompt\r\nfunction answer() return 42 end\r\n```\r\n\r\nDescription.\r\n\r\n## S\r\n\r\np\r\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("immediate shared Lua must parse");
        let shared = prompt.shared.expect("shared program must be present");
        assert_eq!(shared.source(), "function answer() return 42 end");
        assert_eq!(prompt.description_text, "Description.");
    }

    #[test]
    fn shared_library_after_description_is_misplaced() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\nDescription first.\n\n```lua prompt\nreturn 1\n```\n\n## S\n\np\n";
        let error =
            Prompt::parse(src, "test", &NullObserver).expect_err("shared Lua must lead H1 content");
        assert!(error.to_string().contains("immediately follow the H1"));
    }

    #[test]
    fn duplicate_shared_libraries_error() {
        let in_h1 = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua prompt\nlocal a = 1\n```\n\n```lua prompt\nlocal b = 2\n```\n\n## S\n\np\n";
        let error = Prompt::parse(in_h1, "test", &NullObserver)
            .expect_err("a second H1 library must be rejected");
        assert!(error.to_string().contains("more than one"));

        let in_section = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua prompt\nlocal a = 1\n```\n\n## S\n\n```lua prompt\nlocal b = 2\n```\n";
        let error = Prompt::parse(in_section, "test", &NullObserver)
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
                "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro.\n\n{near_miss}\n\n## S\n\np\n"
            );
            let prompt = Prompt::parse(&src, "test", &NullObserver)
                .expect("near-miss shared markers must remain prose");
            assert!(prompt.shared.is_none());
            assert!(prompt.description_text.contains(near_miss));
        }

        let unclosed = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua prompt\nreturn 1\n````\n\n## S\n\np\n";
        let error = Prompt::parse(unclosed, "test", &NullObserver)
            .expect_err("near-miss closing marker must not close the fence");
        assert!(error.to_string().contains("not closed"));
    }

    #[test]
    fn shared_markers_inside_longer_fences_remain_prose() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n````markdown\n```lua prompt\nreturn 1\n```\n````\n\nIntro.\n\n## S\n\n````markdown\n```lua prompt\nreturn 2\n```\n````\n";
        let prompt = Prompt::parse(src, "test", &NullObserver)
            .expect("nested shared markers must remain prose");

        assert!(prompt.shared.is_none());
        assert!(prompt.description_text.contains("```lua prompt"));
        assert!(prompt.sections[0].preamble.is_none());
        assert!(prompt.sections[0].prose.contains("```lua prompt"));
    }

    #[test]
    fn malformed_shared_lua_retains_diagnostics_and_reports_safe_boundaries() {
        let recorder = Recorder::default();
        let source = "private_payload =";
        let src = format!(
            "---\nname: x\ndescription: d\n---\n\n# Private title\n\n```lua prompt\n{source}\n```\n\n## S\n\np\n"
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
        let source = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua prompt\nlocal secret = 42\n```\n\n## S\n\np\n";
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
            p.sections[0].preamble.as_ref().map(LuaProgram::source),
            Some("return 42")
        );
        assert_eq!(p.sections[0].prose, "Actual prose here.");
        assert!(p.sections[0].epilog.is_none());
    }

    #[test]
    fn section_compiles_preamble_and_epilog_around_prose() {
        let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n## Transform\r\n\r\n \t\r\n```lua\r\nvar.before = args\r\n```\r\n\r\nAsk about {{ var.before }}.\r\n\r\n```lua\r\nreturn reply\r\n```\r\n";
        let prompt = Prompt::parse(src, "test", &NullObserver)
            .expect("both exact section phases must compile");
        let section = prompt.entry();

        assert_eq!(
            section.preamble.as_ref().map(LuaProgram::source),
            Some("var.before = args")
        );
        assert_eq!(section.prose, "Ask about {{ var.before }}.");
        assert_eq!(
            section.epilog.as_ref().map(LuaProgram::source),
            Some("return reply")
        );
    }

    #[test]
    fn section_compiles_epilog_after_prose_without_preamble() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Transform\n\nAsk the model.\n\n```lua\nreturn reply\n```\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("the trailing epilog must compile");
        let section = prompt.entry();

        assert!(section.preamble.is_none());
        assert_eq!(section.prose, "Ask the model.");
        assert_eq!(
            section.epilog.as_ref().map(LuaProgram::source),
            Some("return reply")
        );
    }

    #[test]
    fn exact_middle_lua_fences_remain_prose_without_compilation() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nBefore.\n\n```lua\nnot valid lua =\n```\n\nAfter.\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("middle Lua is model-facing prose");
        let section = prompt.entry();

        assert!(section.preamble.is_none());
        assert!(section.epilog.is_none());
        assert_eq!(
            section.prose,
            "Before.\n\n```lua\nnot valid lua =\n```\n\nAfter."
        );
    }

    #[test]
    fn one_exact_fence_is_the_preamble_and_two_can_surround_empty_prose() {
        let one = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.x = 1\n```\n";
        let prompt = Prompt::parse(one, "test", &NullObserver).expect("one fence is the preamble");
        assert!(prompt.entry().preamble.is_some());
        assert!(prompt.entry().epilog.is_none());

        let two = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.x = 1\n```\n\n```lua\nreturn reply\n```\n";
        let prompt =
            Prompt::parse(two, "test", &NullObserver).expect("two fences can enclose empty prose");
        assert_eq!(prompt.entry().prose, "");
        assert!(prompt.entry().preamble.is_some());
        assert!(prompt.entry().epilog.is_some());
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
            assert!(prompt.entry().preamble.is_none());
            assert!(prompt.entry().epilog.is_none());
            assert_eq!(prompt.entry().prose, near_miss.trim());
        }
    }

    #[test]
    fn section_markers_inside_longer_fences_remain_prose() {
        let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n````markdown\n```lua\nreturn 1\n```\n````\n";
        let prompt =
            Prompt::parse(src, "test", &NullObserver).expect("nested markers must remain prose");

        assert!(prompt.entry().preamble.is_none());
        assert!(prompt.entry().epilog.is_none());
        assert!(prompt.entry().prose.contains("```lua"));
    }

    #[test]
    fn malformed_section_phases_report_locations_and_safe_boundaries() {
        for (phase, content, expected_location, expected_details) in [
            (
                "preamble",
                "```lua\nprivate_payload =\n```\n\nProse.",
                "section `Private section` preamble",
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
            ("```lua\nreturn 1", "preamble"),
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
        assert!(p.sections[0].preamble.is_none());
        assert!(p.sections[0].epilog.is_none());
        assert!(p.sections[0].prose.contains("```python"));
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
}
