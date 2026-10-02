//! Crate-wide parser tests: frontmatter, headings, fences, lists, breaks, and line mapping.

use promptforge_types::event::Event;

use super::*;

mod breaks;
mod frontmatter;
mod h1_fences;
mod headings;
mod line_mapping;
mod lists;
mod section_fences;

fn prompt_src(body: &str) -> String {
    format!("---\nname: x\ndescription: d\n---\n\n# T\n\n{body}")
}

#[test]
fn invalid_frontmatter_preserves_the_yaml_cause_as_source() {
    // A malformed-YAML frontmatter must classify as `Frontmatter` and
    // retain the underlying serde_yaml_ng failure as the public error's
    // `source()`, instead of flattening it into a string.
    let src = "---\nname: p\ndescription: d\n: : :\n---\n\n# T\n\n## S\n\nhi\n";
    let error = parse(src).expect_err("malformed YAML frontmatter must fail to parse");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
    assert!(
        std::error::Error::source(&error).is_some(),
        "the YAML decode failure must be preserved as the error source: {error}"
    );
}

#[test]
fn frontmatter_syntax_errors_report_a_position_without_a_name() {
    // Step 6: a malformed-YAML frontmatter surfaces the retained
    // serde_yaml_ng position (1-based, file-absolute); the failure predates
    // the prompt's name, so none is reported.
    let src = "---\nname: p\ndescription: d\n: : :\n---\n\n# T\n\n## S\n\nhi\n";
    let error = parse(src).expect_err("malformed YAML frontmatter must fail to parse");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
    assert_eq!(error.line(), Some(4), "the malformed line: {error}");
    assert!(
        error.column().is_some(),
        "a column accompanies the line: {error}"
    );
    assert_eq!(error.name(), None);
}

#[test]
fn structured_errors_report_the_prompt_name_and_source_position() {
    // Step 6: a structured failure postdates the frontmatter, so the parse
    // error reports the prompt's frontmatter name plus the offending
    // span's 1-based line and column, computed against the source.
    let src = concat!(
        "---\nname: dup\ndescription: d\n---\n", // lines 1-4
        "\n# T\n\n## S\n\np\n\n## S\n\nq\n",     // the second `## S` heads line 12
    );
    let error = parse(src).expect_err("duplicate sibling sections must be rejected");
    assert_eq!(error.kind(), ParseErrorKind::Structure);
    assert_eq!(error.name(), Some("dup"));
    assert_eq!(
        error.line(),
        Some(12),
        "the duplicate heading's line: {error}"
    );
    assert_eq!(
        error.column(),
        Some(1),
        "the duplicate heading's column: {error}"
    );
    assert!(
        error.span().is_some(),
        "the byte span is preserved alongside the line/column"
    );
}

#[test]
fn orphan_empty_heading_and_misplaced_shared_fence_errors_report_a_line() {
    // `prompt_src` puts `# T` on line 6 and the body from line 8.
    for (body, line, kind) in [
        ("## A\n\na\n\n#### D\n\nd\n", 12, ParseErrorKind::Structure),
        ("## \n\na\n", 8, ParseErrorKind::Structure),
        (
            "## S\n\n```lua shared\nlocal a = 1\n```\n",
            10,
            ParseErrorKind::Fence,
        ),
    ] {
        let error = parse(&prompt_src(body)).expect_err("the body must fail to parse");
        assert_eq!(error.kind(), kind, "{error}");
        assert_eq!(error.line(), Some(line), "{error}");
        assert_eq!(error.column(), Some(1), "{error}");
        assert!(error.span().is_some(), "{error}");
    }
}

#[test]
fn parsed_prompt_value_types_are_equatable() {
    // Parsing the same source twice yields equal values, and a differing
    // source yields unequal values, across the finalized parser value types
    // (`Prompt`, `Frontmatter`, `Section`, `Block`).
    let src = "---\nname: p\ndescription: d\n---\n\n# Title\n\n## One\n\ndo a thing\n";
    let a = parse(src).unwrap();
    let b = parse(src).unwrap();
    assert_eq!(a, b, "identical sources must parse equal");
    assert_eq!(a.frontmatter, b.frontmatter);
    assert_eq!(a.sections, b.sections);

    let other = "---\nname: p\ndescription: d\n---\n\n# Title\n\n## Two\n\ndo a thing\n";
    let c = parse(other).unwrap();
    assert_ne!(a, c, "differing section headings must parse unequal");
}

/// Parses under the suite's execution id, keeping the outcome alone.
fn parse(src: &str) -> std::result::Result<Prompt, ParseError> {
    Prompt::parse(src, "test").0
}

/// The parse-time events read back as `(execution, section, kind)`.
struct Recorder(Vec<Event>);

/// The `kind` labels the parse reports.
mod detail {
    pub(super) const PARSE_STARTED: &str = "parse_started";
    pub(super) const PARSE_SUCCEEDED: &str = "parse_succeeded";
    pub(super) const PARSE_FAILED: &str = "parse_failed";
    pub(super) const LUA_COMPILATION_STARTED: &str = "lua_compilation_started";
    pub(super) const LUA_COMPILATION_SUCCEEDED: &str = "lua_compilation_succeeded";
    pub(super) const LUA_COMPILATION_FAILED: &str = "lua_compilation_failed";
}

/// The `kind` label of one of the events a parse reports.
fn kind(event: &Event) -> String {
    match event {
        Event::ParseStarted { .. } => detail::PARSE_STARTED,
        Event::ParseSucceeded { .. } => detail::PARSE_SUCCEEDED,
        Event::ParseFailed { .. } => detail::PARSE_FAILED,
        Event::LuaCompilationStarted { .. } => detail::LUA_COMPILATION_STARTED,
        Event::LuaCompilationSucceeded { .. } => detail::LUA_COMPILATION_SUCCEEDED,
        Event::LuaCompilationFailed { .. } => detail::LUA_COMPILATION_FAILED,
        other => panic!("a parse reports no {other:?}"),
    }
    .to_owned()
}

impl Recorder {
    fn records(&self) -> Vec<(String, String, String)> {
        self.0
            .iter()
            .map(|event| {
                (
                    event.execution().to_owned(),
                    event.section().to_owned(),
                    kind(event),
                )
            })
            .collect()
    }

    fn observations(&self) -> Vec<(String, String)> {
        self.0
            .iter()
            .map(|event| (event.section().to_owned(), kind(event)))
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

    let p = parse(src).unwrap();
    assert_eq!(p.frontmatter.name, "demo");
    assert_eq!(p.frontmatter.description, "A demo");
    assert_eq!(p.title, "Demo Title");
    assert!(p.replay.is_none());
    assert_eq!(
        p.h1_blocks,
        vec![Block::Prose {
            text: "Human-readable intro text.".to_owned(),
        }]
    );
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
    let p = parse(src).unwrap();
    assert_eq!(p.sections.len(), 1);
    assert_eq!(p.sections[0].name, "Greet");
    assert_eq!(p.sections[0].prose(), "Say hi");
}
