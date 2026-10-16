//! Parses a prompt's source into a [`Prompt`]: frontmatter, H1, and sections.

use std::num::NonZeroU32;
use std::sync::Arc;

use runvil_types::emitter::{DebugMode, Emitter, EventSink};
use runvil_types::event::{Event, lifecycle};

use crate::contract::{self, PluginListError};
use crate::fence::{self, FenceViolation};
use crate::sections::{self, Heading};
use crate::{Error, Frontmatter, ParseError, ParseErrorKind, Prompt, Result, frontmatter};

impl Prompt {
    /// Parses the full source text of a prompt file into a [`Prompt`] and
    /// returns the events reported during parsing alongside the result.
    ///
    /// The caller supplies `execution`, an identifier that every event
    /// carries. Every event is reported under task `0` because parsing happens
    /// before any run starts.
    ///
    /// The events come in this order: `ParseStarted`, then a
    /// `LuaCompilationStarted` event and a `LuaCompilationSucceeded` or
    /// `LuaCompilationFailed` event for each Lua block the parser compiles,
    /// then `ParseSucceeded` or `ParseFailed`. They exist only for the caller
    /// to log.
    ///
    /// # Errors
    /// On failure, the first element of the pair is a [`ParseError`] whose
    /// kind is:
    ///
    /// - `Frontmatter` when the `---` delimiters are missing, the frontmatter
    ///   fails to decode, a Plugin entry is not a plain name, or a Plugin is
    ///   declared twice.
    /// - `Structure` when the H1 is missing, there is more than one H1, the H1
    ///   title is empty, a section heading skips a level (such as an H4
    ///   directly under an H2), a section heading is empty, or two sibling
    ///   sections have the same name.
    /// - `Fence` when a `lua` fence is left open, or anywhere in the body a
    ///   fence or line breaks the strict Lua fence rule. A `lua` fence opens
    ///   with a line that is exactly three backticks then `lua` and closes
    ///   with a line that is exactly three backticks. Label: a fence's label
    ///   starts with `lua` in any letter case without being exactly `lua`,
    ///   such as `Lua` or `lua extra`. Opener: a fence labeled `lua` opens
    ///   with any other line, such as one that is indented, uses tildes or
    ///   four backticks, or sits in a block quote or list item. Closer: a
    ///   fence with an exact opening line closes with any other line.
    ///   Lookalike: a line outside every fence looks like a `lua` opening
    ///   line, such as one in an indented code block. The first violation in
    ///   source order fails the parse at its line, before any Lua compiles.
    ///   Lines inside the body of another fenced code block are never
    ///   inspected, so a document can show a Lua fence as an example.
    /// - `List` when a section made up only of list items has an empty item.
    /// - `Lua` when a Lua block in the H1 or in a section fails to compile as
    ///   Lua.
    /// - `Internal` when the parser fails through no fault of the prompt: one
    ///   of its own invariants breaks, or the Lua compiler cannot start.
    pub fn parse(
        input: &str,
        execution: &str,
    ) -> (std::result::Result<Prompt, ParseError>, Vec<Event>) {
        let sink = EventSink::default();
        let emitter = Emitter::root(sink.clone(), execution, DebugMode::Off);
        emitter.report("Prompt", lifecycle::PARSE_STARTED);
        let result = Self::parse_inner(input, &emitter);
        emitter.report(
            "Prompt",
            if result.is_ok() {
                lifecycle::PARSE_SUCCEEDED
            } else {
                lifecycle::PARSE_FAILED
            },
        );
        (result.map_err(ParseError::from_inner), sink.take())
    }

    fn parse_inner(input: &str, emitter: &Emitter) -> Result<Prompt> {
        let (yaml, body, frontmatter_lines) = frontmatter::split_frontmatter(input)?;
        let body: Arc<str> = Arc::from(body);
        let frontmatter: Frontmatter = serde_yaml_ng::from_str(&yaml).map_err(|e| {
            // The location is relative to the frontmatter block, which starts
            // on file line 2 (line 1 is the opening `---` delimiter).
            let (line, column) = e.location().map_or((None, None), |location| {
                (
                    u32::try_from(location.line())
                        .ok()
                        .and_then(|line| line.checked_add(1))
                        .and_then(NonZeroU32::new),
                    u32::try_from(location.column())
                        .ok()
                        .and_then(NonZeroU32::new),
                )
            });
            Error::ParseFrontmatter {
                message: e.to_string(),
                cause: Box::new(e),
                line,
                column,
            }
        })?;
        contract::check_distinct_plugins(frontmatter.plugins()).map_err(
            |refusal| match refusal {
                PluginListError::Duplicate(_) => {
                    Error::parse(ParseErrorKind::Frontmatter, refusal.to_string())
                }
            },
        )?;
        // Everything past the frontmatter postdates the prompt's name, so a
        // failure from here on is stamped with it (and its span's position).
        let name = frontmatter.name().to_owned();
        Self::parse_body(frontmatter, &body, frontmatter_lines, emitter)
            .map_err(|error| error.with_prompt_context(&name, &body, frontmatter_lines))
    }

    fn parse_body(
        frontmatter: Frontmatter,
        shared: &Arc<str>,
        frontmatter_lines: u32,
        emitter: &Emitter,
    ) -> Result<Prompt> {
        let body: &str = shared;
        let headings = sections::collect_headings(body)?;

        let h1_positions: Vec<usize> = headings
            .iter()
            .enumerate()
            .filter_map(|(index, heading)| (heading.level == 1).then_some(index))
            .collect();
        let [h1_index] = h1_positions.as_slice() else {
            if h1_positions.is_empty() {
                return Err(Error::parse(
                    ParseErrorKind::Structure,
                    "prompt requires an H1 title",
                ));
            }
            let lines = h1_positions
                .iter()
                .map(|&index| {
                    sections::line_add(frontmatter_lines, headings[index].source_line)
                        .map(|line| line.to_string())
                })
                .collect::<Result<Vec<_>>>()?;
            return Err(Error::parse(
                ParseErrorKind::Structure,
                format!(
                    "prompt must contain exactly one H1 title, found {} at lines {}",
                    lines.len(),
                    lines.join(", ")
                ),
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
        let h1_content_abs_line = sections::line_add(frontmatter_lines, h1.content_start_line)?;
        if let Some((line, violation)) = fence::first_fence_violation(body) {
            let message = match violation {
                FenceViolation::Label(label) => {
                    format!("unrecognized fence label `{label}`; the only Lua fence label is `lua`")
                }
                FenceViolation::Opener => {
                    "`lua` fence opening line must be exactly ```lua".to_owned()
                }
                FenceViolation::Closer => "`lua` fence closing line must be exactly ```".to_owned(),
            };
            return Err(Error::parse_at(ParseErrorKind::Fence, line, message));
        }
        let (h1_blocks, description_text) =
            fence::split_h1(shared, h1.content, &title, h1_content_abs_line, emitter)?;

        // Everything before the H1 is preface and has no prompt semantics.
        // Sections are headings after the H1 at level 2 or deeper.
        let section_headings: Vec<Heading<'_>> = headings
            .into_iter()
            .skip(*h1_index + 1)
            .filter(|h| h.level >= 2)
            .collect();
        let mut pos = 0;
        let sections = sections::build_sections(
            shared,
            &section_headings,
            &mut pos,
            1,
            frontmatter_lines,
            emitter,
        )?;

        Ok(Prompt {
            frontmatter,
            title,
            h1_blocks,
            description_text,
            sections,
        })
    }
}
