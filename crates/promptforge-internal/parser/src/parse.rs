//! The parse entry point: frontmatter decoding, H1 validation, and the
//! assembly of a [`Prompt`] from its headings, fences, and sections.

use promptforge_types::emitter::{DebugMode, Emitter, EventSink};
use promptforge_types::event::{Event, lifecycle};

use crate::build::{
    Frontmatter, Heading, build_sections, collect_headings, line_add, split_frontmatter,
};
use crate::fence::{exact_shared_openings, split_h1};
use crate::{Error, ParseError, ParseErrorKind, Prompt, Result};

impl Prompt {
    /// Parses a prompt file's full source text into a [`Prompt`], returning
    /// the parse-time events beside the outcome.
    ///
    /// The events are the parse lifecycle (`ParseStarted`, then
    /// `ParseSucceeded` or `ParseFailed`) and each Lua block's compilation
    /// boundaries, every one stamped with the caller-provided `execution`
    /// identifier and reported under task `0`, since no run exists yet.
    /// They are values for the caller to log; nothing is read back.
    ///
    /// ```
    /// use promptforge::event::Event;
    /// use promptforge::{ParseErrorKind, Prompt};
    ///
    /// let source = "---\nname: greeter\ndescription: says hi\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n";
    /// let (prompt, events) = Prompt::parse(source, "docs");
    /// let prompt = prompt?;
    /// assert_eq!(prompt.frontmatter().name(), "greeter");
    /// assert_eq!(prompt.title(), "Greeter");
    /// assert!(matches!(events.first(), Some(Event::ParseStarted { .. })));
    /// assert!(matches!(events.last(), Some(Event::ParseSucceeded { .. })));
    ///
    /// // A malformed prompt reports a classified error, and the events say so.
    /// let (err, events) = Prompt::parse("no frontmatter here", "docs");
    /// assert_eq!(err.unwrap_err().kind(), ParseErrorKind::Frontmatter);
    /// assert!(matches!(events.last(), Some(Event::ParseFailed { .. })));
    /// # Ok::<(), promptforge::ParseError>(())
    /// ```
    ///
    /// # Errors
    /// The first half of the pair is a [`ParseError`] classified `Frontmatter` when the frontmatter
    /// delimiters are missing or the frontmatter is invalid; `Structure` when
    /// the required H1 is missing or the body has no `##` sections; `Fence` when
    /// the H1 opens with the removed `lua prompt` fence form, an exact fence
    /// is not closed, more than one `lua shared` fence exists, or a
    /// `lua shared` fence is outside H1; and `Lua` when the shared library or an
    /// H1 or section Lua block is not valid Lua.
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
        let (yaml, body, frontmatter_lines) = split_frontmatter(input)?;
        let frontmatter: Frontmatter = serde_yaml_ng::from_str(&yaml).map_err(|e| {
            // Retain the YAML decode failure as the `#[source]` cause (F3) and
            // surface its location, so the public parse error exposes the
            // frontmatter syntax position as stored fields. The location is
            // relative to the frontmatter block, which starts on file line 2
            // (line 1 is the opening `---` delimiter).
            let (line, column) = e.location().map_or((None, None), |location| {
                (
                    u32::try_from(location.line())
                        .ok()
                        .and_then(|line| line.checked_add(1)),
                    u32::try_from(location.column()).ok(),
                )
            });
            Error::ParseFrontmatter {
                message: e.to_string(),
                source: Box::new(e),
                line,
                column,
            }
        })?;
        // Everything past the frontmatter postdates the prompt's name, so a
        // failure from here on is stamped with it (and its span's position).
        let name = frontmatter.name().to_owned();
        Self::parse_body(frontmatter, &body, frontmatter_lines, emitter)
            .map_err(|error| error.with_prompt_context(&name, &body, frontmatter_lines))
    }

    fn parse_body(
        frontmatter: Frontmatter,
        body: &str,
        frontmatter_lines: u32,
        emitter: &Emitter,
    ) -> Result<Prompt> {
        let headings = collect_headings(body)?;

        let h1_positions: Vec<usize> = headings
            .iter()
            .enumerate()
            .filter_map(|(index, heading)| (heading.level == 1).then_some(index))
            .collect();
        let [h1_index] = h1_positions.as_slice() else {
            return Err(Error::parse(
                ParseErrorKind::Structure,
                if h1_positions.is_empty() {
                    "prompt requires an H1 title"
                } else {
                    "prompt must contain exactly one H1 title"
                },
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
        let h1_content_abs_line = line_add(frontmatter_lines, h1.content_start_line)?;
        let shared_fences = exact_shared_openings(body);
        let h1_shared_fences = exact_shared_openings(&h1.content);
        if shared_fences.len() > 1 {
            return Err(Error::parse(
                ParseErrorKind::Fence,
                "prompt allows at most one `lua shared` fence",
            ));
        }
        if shared_fences.len() != h1_shared_fences.len() {
            return Err(Error::parse(
                ParseErrorKind::Fence,
                "`lua shared` fence is allowed only in H1",
            ));
        }
        let (replay, h1_blocks, description_text) =
            split_h1(&h1.content, &title, h1_content_abs_line, emitter)?;

        // Everything before the H1 is preface and has no prompt semantics.
        // Sections are headings after the H1 at level 2 or deeper.
        let section_headings: Vec<Heading> = headings
            .into_iter()
            .skip(*h1_index + 1)
            .filter(|h| h.level >= 2)
            .collect();
        let mut pos = 0;
        let sections = build_sections(&section_headings, &mut pos, 1, frontmatter_lines, emitter)?;

        Ok(Prompt {
            frontmatter,
            title,
            replay,
            h1_blocks,
            description_text,
            sections,
        })
    }
}
