//! Test support for the parser's own tests and, through the `test-support`
//! feature, for companion crates' tests.
//!
//! `Section` and `Block::Prose` are `#[non_exhaustive]`, so a companion
//! crate's tests cannot construct them literally; these constructors are the
//! cross-crate seam for synthetic section trees. The source builder writes a
//! prompt from its parts, and the parse assertions show the source and the
//! actual outcome when a parse goes the other way.

use crate::{Block, BodyText, ParseError, ParseErrorKind, Prompt, Section};

/// Builds a synthetic section with the given blocks, so each test fixture
/// states only its own deltas (a prose block) instead of restating the
/// parser's `Section` literal.
#[must_use]
pub fn synthetic_section(name: &str, level: u8, blocks: Vec<Block>) -> Section {
    Section {
        name: name.to_string(),
        level,
        blocks,
        children: Vec::new(),
        items: Vec::new(),
        markdown: BodyText::from_owned(String::new()),
    }
}

/// Builds a prose block, the one `Block` variant the executor's test
/// fixtures need to construct directly.
#[must_use]
pub fn prose_block(text: String) -> Block {
    Block::Prose {
        text: BodyText::from_owned(text),
    }
}

/// Writes the prompt `t` with one `lua`-fenced section per `(heading, lua)`
/// entry.
///
/// The frontmatter has `name: t`, `description: d`, and `runvil: 0`,
/// then the `front` lines, then `# Test prompt` and the sections. A heading
/// that starts with `#` is written as given, and any other heading becomes an
/// H2. Every line is trimmed, so `front` and each body can be indented raw
/// strings.
#[must_use]
pub fn prompt(front: &str, sections: &[(&str, &str)]) -> String {
    let mut source = PromptSource::new().front(front);
    for (heading, lua) in sections {
        source = source.section(heading, lua);
    }
    source.build()
}

/// The prompt `t` with one section, `Only`, whose body is `lua`.
#[must_use]
pub fn one_section(lua: &str) -> String {
    prompt("", &[("Only", lua)])
}

/// A prompt source built part by part, for shapes [`prompt`] cannot express.
///
/// Those shapes are another name or title, H1 blocks, and prose sections. It
/// has [`prompt`]'s defaults and trimming.
#[derive(Debug, Clone)]
#[must_use]
pub struct PromptSource {
    name: String,
    title: String,
    front: String,
    h1: String,
    sections: String,
}

impl Default for PromptSource {
    fn default() -> Self {
        Self::new()
    }
}

impl PromptSource {
    /// The prompt `t`, titled `Test prompt`, with an empty H1 and no sections.
    pub fn new() -> Self {
        Self {
            name: "t".to_owned(),
            title: "Test prompt".to_owned(),
            front: String::new(),
            h1: String::new(),
            sections: String::new(),
        }
    }

    /// Replaces the frontmatter `name`.
    pub fn name(mut self, name: &str) -> Self {
        name.clone_into(&mut self.name);
        self
    }

    /// Replaces the H1 title.
    pub fn title(mut self, title: &str) -> Self {
        title.clone_into(&mut self.title);
        self
    }

    /// Adds frontmatter lines after the fixed ones.
    pub fn front(mut self, front: &str) -> Self {
        self.front += &trimmed(front);
        self
    }

    /// Adds a `lua` fence holding `lua` to the H1.
    pub fn h1(self, lua: &str) -> Self {
        self.h1_prose(&fence(lua))
    }

    /// Adds the Markdown `text` to the H1.
    pub fn h1_prose(mut self, text: &str) -> Self {
        self.h1.push('\n');
        self.h1 += &trimmed(text);
        self
    }

    /// Adds a section whose body is one `lua` fence holding `lua`.
    pub fn section(self, heading: &str, lua: &str) -> Self {
        self.prose(heading, &fence(lua))
    }

    /// Adds a section whose body is the Markdown `text`, with no fence added.
    pub fn prose(mut self, heading: &str, text: &str) -> Self {
        let marker = if heading.starts_with('#') { "" } else { "## " };
        self.sections += &["\n", marker, heading, "\n\n", &trimmed(text)].concat();
        self
    }

    /// The source text.
    #[must_use]
    pub fn build(&self) -> String {
        let head = format!("---\nname: {}\ndescription: d\nrunvil: 0\n", self.name);
        let h1 = format!("{}---\n\n# {}\n{}", self.front, self.title, self.h1);
        head + &h1 + &self.sections
    }
}

fn fence(lua: &str) -> String {
    format!("```lua\n{}```", trimmed(lua))
}

fn trimmed(text: &str) -> String {
    let lines = text.trim().lines().map(str::trim);
    lines.flat_map(|line| [line, "\n"]).collect()
}

/// Parses `src` under the execution id `test`.
///
/// # Panics
///
/// When `src` fails to parse, naming the error and showing the source's
/// first lines.
#[track_caller]
#[expect(
    clippy::must_use_candidate,
    reason = "a test may call it only to assert that the source parses"
)]
pub fn parse_ok(src: &str) -> Prompt {
    match Prompt::parse(src, "test").0 {
        Ok(prompt) => prompt,
        Err(error) => panic!("expected a parse, got {error:?}\n{}", excerpt(src)),
    }
}

/// Parses `src` under the execution id `test`, expecting it to fail.
///
/// # Panics
///
/// When `src` parses, naming its title and showing the source's first lines.
#[track_caller]
#[must_use]
pub fn parse_err(src: &str) -> ParseError {
    match Prompt::parse(src, "test").0 {
        Ok(prompt) => panic!(
            "expected an error, parsed `{}`\n{}",
            prompt.title,
            excerpt(src)
        ),
        Err(error) => error,
    }
}

/// Asserts that `src` fails to parse with a `kind` error whose message
/// contains `fragment`.
///
/// # Panics
///
/// When `src` parses, or fails with another kind or message, naming the
/// actual error and showing the source's first lines.
#[track_caller]
pub fn assert_parse_error(src: &str, kind: ParseErrorKind, fragment: &str) {
    let error = parse_err(src);
    let expected = format!("expected a {kind:?} error containing {fragment:?}");
    let matched = error.kind() == kind && error.to_string().contains(fragment);
    assert!(matched, "{expected}, got {error:?}\n{}", excerpt(src));
}

/// The first lines of `src`, for a failure message.
fn excerpt(src: &str) -> String {
    let shown: Vec<&str> = src.lines().take(12).collect();
    format!("source, first {} lines:\n{}", shown.len(), shown.join("\n"))
}
